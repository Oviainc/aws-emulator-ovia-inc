Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.put-rest-api-overwrite-replaces-definition apigateway.base-path-mapping-duplicate-basepath-rejected apigateway.concurrent-create-base-path-mapping-race apigateway.concurrent-delete-domain-name-access-association-race apigateway.concurrent-domain-name-access-association-duplicate apigateway.create-base-path-mapping-nonexistent-stage-rejected apigateway.create-base-path-mapping-unknown-restapi-returns-badrequest apigateway.create-domain-name-access-association-without-vpce-fails apigateway.create-domain-name-duplicate-conflict apigateway.create-domain-name-malformed-certificate-arn-rejected apigateway.create-duplicate-domain-name-conflict apigateway.custom-domain-basepath-remap-flow apigateway.delete-domain-name-with-active-association-succeeds apigateway.delete-domain-name-with-active-basepath-mapping-conflict apigateway.delete-domain-name-with-dependent-base-path-mapping-conflict apigateway.delete-domain-name-with-mapping-rejected apigateway.delete-nonexistent-domain-name-returns-notfound apigateway.deploy-custom-domain-cfn-mapping-active apigateway.deploy-domain-name-basepathmapping-cfn-resolves apigateway.deploy-domain-name-cfn-behaves apigateway.domain-access-association-concurrent-reject-delete apigateway.domain-name-lifecycle apigateway.domain-name-regional-provisioning-status-window apigateway.e2e-custom-domain-base-path-routing apigateway.e2e-custom-domain-basepath-routing-consequence apigateway.e2e-domain-name-basepath-mapping-routes-to-stage apigateway.get-base-path-mapping-not-found-vs-badrequest apigateway.get-domain-names-lists-created apigateway.reject-domain-name-access-association-wrong-state-fails apigateway.update-domain-name-immutable-field-rejected apigateway.update-domain-name-invalid-security-policy-bad-request apigateway.update-domain-name-invalid-security-policy-rejected acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one apigateway.concurrent-create-base-path-mapping-duplicate-race apigateway.concurrent-create-domain-name-access-association-race apigateway.create-base-path-mapping-duplicate-rejected apigateway.domain-name-access-association-lifecycle apigateway.reject-domain-name-access-association-removes-pending apigateway.delete-vpc-link-in-use-fails
Issue-Key: api-gateway|GetIntegration|NotFoundException
Issue-Key: apigateway|ImportCertificate|NotImplemented
Issue-Key: acm__apigateway|ImportCertificate|NotImplemented
Issue-Key: apigateway|CreateVpcEndpoint|NotImplemented
Issue-Key: apigateway.delete-vpc-link-in-use-fails|run#8|KeyError|VpcId=elbv2.describe_load_balancers(LoadBalancerArns=[nlb_arn])["LoadBalancers"][0]["VpcId"],
Issue-Commit: e2dfe022d58b39493003b3d90859f84840a12f00

# API Gateway: PutRestApi(overwrite) imports paths but no integrations, and the whole v1 custom-domain / access-association surface is unreachable (ACM ImportCertificate and EC2 CreateVpcEndpoint are 501 stubs, ELBv2 DescribeLoadBalancers omits VpcId)

> _Note: this issue was identified against the code base at commit `e2dfe022d58b39493003b3d90859f84840a12f00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRestApi

## Description
Four root causes that block API Gateway (REST, v1) behavioral surfaces. Causes 2-4 are defects in OTHER services' crates that are reached only through these apigateway checks, so they are reported here with the apigateway checks that carry their replay keys. A fixer must close ALL FOUR before submitting.

Cause 1 of 4: PutRestApi with mode='overwrite' imports the OpenAPI/Swagger paths and methods but not their x-amazon-apigateway-integration, so GetIntegration on an imported method answers 404.
AWS: Smithy (com.amazonaws.apigateway#PutRestApi): PUT /restapis/{restApiId} with mode=overwrite, success 200 returning RestApi. The imported document's x-amazon-apigateway-integration extension is materialized as a real Integration on each imported method, so GetIntegration(restApiId, resourceId, httpMethod) answers 200 with type='MOCK' for a document declaring a MOCK integration. GetIntegration models NotFoundException(404) only when no integration exists.
Emulator: after a successful PutRestApi(mode='overwrite') whose document declares /replaced GET with a MOCK x-amazon-apigateway-integration, the resource and its GET method are present, but GetIntegration for that method answers HTTP 404 NotFoundException ('Invalid Integration identifier specified').
Why: a client that provisions an API from an OpenAPI document gets a method with no integration — every request to it would fail at the integration step, and tooling that reads the integration back to verify the import sees a half-imported API.
Fix: the PutRestApi handler's import path (services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/put_rest_api.rs) and the integration state it must write.
Traces: apigateway.put-rest-api-overwrite-replaces-definition.

Cause 2 of 4: ACM ImportCertificate answers 501 NotImplemented, which makes the ENTIRE API Gateway v1 custom-domain surface unreachable (31 apigateway checks plus 1 acm__apigateway check in this suite).
AWS: recorded behavior — ImportCertificate returns a CertificateArn for a self-signed leaf+chain, and that ISSUED certificate ARN is what CreateDomainName/UpdateDomainName require; API Gateway refuses a certificate that is not ISSUED with BadRequestException ('Certificate is not in an ISSUED state, and cannot be used').
Emulator: ImportCertificate answers HTTP 501 Error.Code=NotImplemented ('operation ImportCertificate is scaffolded but not implemented in this emulator'), so no check can obtain a usable certificate and the apigateway domain-name, base-path-mapping, custom-domain routing, domain-name lifecycle and CFN domain-name checks all die before touching API Gateway.
Why: no client can create an API Gateway custom domain at all, so base-path mappings, custom-domain routing, security-policy validation and the DomainName lifecycle are all unobservable.
Fix: services/service-com-amazonaws-acm-certificatemanager/src/ops/import_certificate.rs. Related open issues that do NOT carry these apigateway replay keys: 20261022T143245Z-acm-importcertificate-stub.md (the stub itself) and 20261023T125319Z-acm-importcertificate-501-stub-makes-the-whole-apigatewayv2.md (the same blockage for apigatewayv2). This cause records the v1 checks to re-replay.
Traces: the 31 apigateway custom-domain checks and acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one listed in trace_ids.

Cause 3 of 4: EC2 CreateVpcEndpoint answers 501 NotImplemented, which makes the API Gateway private-custom-domain access-association surface unreachable (5 checks).
AWS: recorded behavior — CreateVpcEndpoint returns a VpcEndpoint with an endpoint id; that id is the required input of CreateDomainNameAccessAssociation, whose association can then be listed, rejected and deleted.
Emulator: CreateVpcEndpoint answers HTTP 501 Error.Code=NotImplemented, so every domain-name-access-association check fails before reaching API Gateway.
Why: private custom domains cannot be associated with a VPC endpoint, so the whole access-association lifecycle (create / duplicate conflict / reject / delete) is unobservable.
Fix: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs. Related open issues that do NOT carry these apigateway replay keys: 20261022T235108Z-ec2-createvpcendpoint-stub.md and 20261023T113357Z-ec2-createvpcendpoint-is-an-unimplemented-scaffold-stub.md.
Traces: apigateway.concurrent-create-base-path-mapping-duplicate-race, apigateway.concurrent-create-domain-name-access-association-race, apigateway.create-base-path-mapping-duplicate-rejected, apigateway.domain-name-access-association-lifecycle, apigateway.reject-domain-name-access-association-removes-pending.

Cause 4 of 4: ELBv2 DescribeLoadBalancers returns a LoadBalancer object with no VpcId member.
AWS: recorded behavior — DescribeLoadBalancers returns each LoadBalancer with VpcId (the VPC the load balancer was created in); it is a normal, always-present member for a VPC load balancer and callers key VPC-scoped resources off it.
Emulator: the LoadBalancer in the response has no 'VpcId' key at all, so a caller reading response['LoadBalancers'][0]['VpcId'] raises KeyError. The apigateway VPC-link check cannot build the VPC link that the rest of the check needs.
Why: any client that resolves an NLB's VPC from DescribeLoadBalancers (to create a VPC link, a VPC endpoint, or a security group) breaks; a missing modeled member is a silent shape divergence, not an error the client can handle.
Fix: the DescribeLoadBalancers response serializer / load-balancer state in services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10 (the created load balancer must retain and report its VpcId).
Traces: apigateway.delete-vpc-link-in-use-fails.

## Evidence
From the behavioral replay (suite simparity-behavioral-aws-v1):
- apigateway.put-rest-api-overwrite-replaces-definition -> botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetIntegration operation: Invalid Integration identifier specified (raised AFTER PutRestApi(mode='overwrite') succeeded and the imported /replaced GET method was visible). Expected: GetIntegration 200 with type='MOCK'.
- 31 apigateway checks + acm__apigateway.regional-custom-domain-... -> botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportCertificate operation: operation ImportCertificate is scaffolded but not implemented in this emulator. Expected: an ISSUED certificate ARN usable by CreateDomainName.
- 5 apigateway checks -> botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint is scaffolded but not implemented in this emulator. Expected: a VpcEndpoint id usable by CreateDomainNameAccessAssociation.
- apigateway.delete-vpc-link-in-use-fails -> KeyError: 'VpcId' at elbv2.describe_load_balancers(LoadBalancerArns=[nlb_arn])['LoadBalancers'][0]['VpcId']. Expected: VpcId present on the returned LoadBalancer.

## Steps to Reproduce
1. Boot the emulator; point botocore at it (us-west-2).
2. Cause 1: create a REST API with a /orig GET MOCK integration, then PutRestApi(restApiId, mode='overwrite', body=<swagger doc declaring /replaced GET with x-amazon-apigateway-integration type MOCK>); GetResources shows '/replaced' with a GET method, but GetIntegration(restApiId, resourceId=<replaced>, httpMethod='GET') answers 404 NotFoundException.
3. Cause 2: call acm.ImportCertificate with a self-signed leaf/key/chain -> 501 NotImplemented; consequently CreateDomainName can never be reached.
4. Cause 3: call ec2.CreateVpcEndpoint for an execute-api interface endpoint -> 501 NotImplemented; consequently CreateDomainNameAccessAssociation can never be reached.
5. Cause 4: create an internal NLB via elbv2.CreateLoadBalancer, then elbv2.DescribeLoadBalancers(LoadBalancerArns=[arn]) and read ['LoadBalancers'][0]['VpcId'] -> KeyError.
Re-replay every check id in trace_ids to confirm.
