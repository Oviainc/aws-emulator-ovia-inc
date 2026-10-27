Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.asvpce-delete-stack-leaves-streaming-vpce-in-place appstream__ec2.asvpce-update-stack-attributes-to-delete-removes-access-endpoints ec2.asvpce-private-dns-vpce-without-dns-hostnames-rejected ec2.asvpce-streaming-service-catalogue-declares-interface-only-full-access ec2.asvpce-streaming-vpce-without-security-groups-adopts-vpc-default-group ec2.ec2-iot-data-interface-endpoint-pins-ipv4-and-zonal-dns-entries ec2.op-contract-describe-vpc-endpoint-services
Issue-Key: ec2|DescribeVpcEndpointServices|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 DescribeVpcEndpointServices: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeVpcEndpointServices

## Description
`DescribeVpcEndpointServices` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `DescribeVpcEndpointServices` call, so each one unblocks only when `DescribeVpcEndpointServices` works: `appstream__ec2.asvpce-delete-stack-leaves-streaming-vpce-in-place`, `appstream__ec2.asvpce-update-stack-attributes-to-delete-removes-access-endpoints`, `ec2.asvpce-private-dns-vpce-without-dns-hostnames-rejected`, `ec2.asvpce-streaming-service-catalogue-declares-interface-only-full-access`, `ec2.asvpce-streaming-vpce-without-security-groups-adopts-vpc-default-group`, `ec2.ec2-iot-data-interface-endpoint-pins-ipv4-and-zonal-dns-entries`, `ec2.op-contract-describe-vpc-endpoint-services`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_vpc_endpoint_services.rs`. Replaying the corpus, the emulator refused the `DescribeVpcEndpointServices` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeVpcEndpointServices operation: operation DescribeVpcEndpointServices is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeVpcEndpointServices and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
