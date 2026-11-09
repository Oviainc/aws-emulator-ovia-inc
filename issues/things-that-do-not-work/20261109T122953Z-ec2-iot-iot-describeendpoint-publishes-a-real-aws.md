Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive
Issue-Key: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive|_publish#31|PublishedUrlRejected|_url = ctx.endpoint_url(host, aws_port=8443)
Issue-Commit: ec57509239c646de823e7950d2544f6a600b68b2

# ec2__iot: iot DescribeEndpoint publishes a real-AWS data hostname, so the VPC-endpoint-conditioned mTLS publish path cannot be reached

> _Note: this issue was identified against the code base at commit `ec57509239c646de823e7950d2544f6a600b68b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoint

## Description
Cause 1 of 1: DescribeEndpoint announces an address outside the emulator.
AWS: DescribeEndpoint (Smithy: GET /endpoint, query endpointType, output endpointAddress; errors InvalidRequestException, ThrottlingException, UnauthorizedException, InternalFailureException) returns the host a client must then address for that endpoint type; for iot:Data-ATS that host serves the HTTPS publish/shadow data plane. The published address is served by the provider that published it.
Emulator: it publishes '<id>-ats.iot.<region>.amazonaws.com' — a real AWS host it does not serve — so the data-plane call is refused before a request is made ('its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://<own host>:<port>/<path>'). The generated id, host and port differ per run and are not the requirement; the requirement is that endpointAddress names a listener the emulator answers on for the requested endpointType.
Why: the behavior under test — whether an IoT policy whose condition pins aws:SourceVpce to a VPC endpoint that no longer exists is enforced or is over-permissive — is about an mTLS publish to the data endpoint. With the published host outside the emulator, that publish never reaches the emulator, so the policy-condition evaluation is unobservable and a client following the documented discovery path is sent to real AWS.
Fix: the DescribeEndpoint handler in the iot service (endpointType-to-published-address resolution) must publish the emulator's own host:port, and the iot-data listener must answer there. The open issue 20261109T113725Z-iot-describeendpoint-publishes-a-real-aws-hostname-id.md names the same root cause but does not carry this check's replay key.
Traces: ec2__iot.iot-dangling-sourcevpce-reference-accepted-over-permissive.

## Evidence
Expected: DescribeEndpoint(endpointType='iot:Data-ATS') returns an address the emulator serves, so the mTLS publish under the aws:SourceVpce-conditioned policy can be attempted and judged. Actual: simparity.published.PublishedUrlRejected: endpoint_url was given https://<id>-ats.iot.<region>.amazonaws.com (published by the emulator or built by the check): its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Create a thing, certificate and an IoT policy whose statement condition pins aws:SourceVpce, then delete the referenced VPC endpoint.
3. Call iot DescribeEndpoint with endpointType 'iot:Data-ATS' and attempt the mTLS publish against the returned endpointAddress; the published host is a real amazonaws.com address the emulator does not serve.
