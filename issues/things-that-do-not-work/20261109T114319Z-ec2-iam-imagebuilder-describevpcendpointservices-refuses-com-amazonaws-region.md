Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__imagebuilder.ibp-distributed-amis-are-region-scoped-and-outlive-the-deleted-image
Issue-Key: ec2|DescribeVpcEndpointServices|InvalidServiceName
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# ec2__iam__imagebuilder: DescribeVpcEndpointServices refuses com.amazonaws.<region>.imagebuilder with InvalidServiceName — the endpoint-service catalog has no Image Builder entry

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpointServices

## Description
Cause 1 of 1: EC2's VPC endpoint-service catalog does not contain the Image Builder interface endpoint service, so DescribeVpcEndpointServices refuses any request naming it and no PrivateLink-only build network can be described or built. Failing check: ec2__iam__imagebuilder.ibp-distributed-amis-are-region-scoped-and-outlive-the-deleted-image.

AWS: recorded behavior only — Smithy (services/service-com-amazonaws-ec2-amazonec2) models DescribeVpcEndpointServices(ServiceNames: list<String>, Filters, ServiceRegions, MaxResults, NextToken) -> DescribeVpcEndpointServicesResult{ServiceNames (serviceNameSet), ServiceDetails (serviceDetailSet), NextToken} and declares no modeled errors, so the catalog contents are AWS's contract. AWS offers an interface endpoint service for Image Builder named com.amazonaws.<region>.imagebuilder in the commercial regions (alongside ssm, ssmmessages, ec2messages and logs, which this same call resolves successfully), so a request listing it answers HTTP 200 with a ServiceDetails entry whose ServiceName echoes the requested name and whose AvailabilityZones list the zones that offer it. AWS's InvalidServiceName refusal — code InvalidServiceName, HTTP 400, message "The Vpc Endpoint Service '<service-name>' does not exist" (fixed text around the quoted name, which is taken verbatim from the request) — applies only to a name no endpoint service has.
Emulator: DescribeVpcEndpointServices(ServiceNames=['com.amazonaws.us-east-1.imagebuilder', 'com.amazonaws.us-east-1.ssm', 'com.amazonaws.us-east-1.ssmmessages', 'com.amazonaws.us-east-1.ec2messages', 'com.amazonaws.us-east-1.logs']) is refused in full with HTTP 400 InvalidServiceName "The Vpc Endpoint Service 'com.amazonaws.us-east-1.imagebuilder' does not exist". Because the call is how the check picks a zone offering every required endpoint, the whole scenario stops there: the VPC, private subnet, gateway and interface endpoints, the build, and the region-scoped AMI distribution assertions are never reached.
Why: any client that builds a no-internet-route VPC for EC2 Image Builder (the documented PrivateLink pattern) cannot even enumerate the services it must create endpoints for, and a request mixing a missing name with valid ones loses all of them because the refusal is whole-request. A caller reads 'does not exist' as 'this region does not offer Image Builder over PrivateLink', which is wrong.
Fix: EC2's VPC endpoint-service catalog/state behind the DescribeVpcEndpointServices read path (and the CreateVpcEndpoint ServiceName resolution that consumes it), so com.amazonaws.<region>.imagebuilder exists as an Interface endpoint service with its availability zones. Related but distinct: open issue 20261108T143229Z-ec2-static-describe-catalogs-serve-synthesized-hardware-and.md covers the FIELD fidelity of endpoint-service details (zone list, privateDnsName*, serviceId format) for services the catalog does have; it does not cover a service name missing from the catalog entirely.
Traces: ec2__iam__imagebuilder.ibp-distributed-amis-are-region-scoped-and-outlive-the-deleted-image.

## Evidence
Expected (AWS): ec2.describe_vpc_endpoint_services(ServiceNames=['com.amazonaws.us-east-1.imagebuilder', 'com.amazonaws.us-east-1.ssm', 'com.amazonaws.us-east-1.ssmmessages', 'com.amazonaws.us-east-1.ec2messages', 'com.amazonaws.us-east-1.logs']) -> HTTP 200 with one ServiceDetails entry per requested name, each carrying ServiceName and a non-empty AvailabilityZones list; the check then intersects those zones with the zones offering its build instance type.
Actual (emulator): botocore.exceptions.ClientError: An error occurred (InvalidServiceName) when calling the DescribeVpcEndpointServices operation: The Vpc Endpoint Service 'com.amazonaws.us-east-1.imagebuilder' does not exist — HTTP 400, no ServiceDetails at all (the four valid names are dropped with it).
Fixed text: "The Vpc Endpoint Service '" + <requested name> + "' does not exist" with code InvalidServiceName; the quoted name is request-derived and the region in it is per-environment. The divergence is that com.amazonaws.<region>.imagebuilder is treated as nonexistent.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Run ec2__iam__imagebuilder.ibp-distributed-amis-are-region-scoped-and-outlive-the-deleted-image; its _zone() helper calls DescribeVpcEndpointServices with ServiceNames=[f'com.amazonaws.us-east-1.{s}' for s in ('imagebuilder','ssm','ssmmessages','ec2messages','logs')].
3. Observe HTTP 400 InvalidServiceName for 'com.amazonaws.us-east-1.imagebuilder' instead of a 200 whose ServiceDetails includes it.
4. Equivalently, call DescribeVpcEndpointServices(ServiceNames=['com.amazonaws.<region>.imagebuilder']) alone, and CreateVpcEndpoint(VpcEndpointType='Interface', ServiceName='com.amazonaws.<region>.imagebuilder', ...); both must succeed.
