Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: directconnect__ec2.gateway-association-reflects-attached-vpc-cidr directconnect__ec2.second-gateway-association-to-same-vgw-rejected-until-first-clears
Issue-Key: direct-connect|CreateDirectConnectGatewayAssociation|NotImplemented
Issue-Commit: 70b7c68145c683ae49b96a2a3a7ef8edad56294d

# Implement direct-connect CreateDirectConnectGatewayAssociation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `70b7c68145c683ae49b96a2a3a7ef8edad56294d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDirectConnectGatewayAssociation

## Description
`CreateDirectConnectGatewayAssociation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-directconnect-overtureservice/src/ops/create_direct_connect_gateway_association.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateDirectConnectGatewayAssociation` call, so each one unblocks only when `CreateDirectConnectGatewayAssociation` works: `directconnect__ec2.gateway-association-reflects-attached-vpc-cidr`, `directconnect__ec2.second-gateway-association-to-same-vgw-rejected-until-first-clears`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-directconnect-overtureservice/src/ops/create_direct_connect_gateway_association.rs`. Replaying the corpus, the emulator refused the `CreateDirectConnectGatewayAssociation` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDirectConnectGatewayAssociation operation: operation CreateDirectConnectGatewayAssociation is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the direct-connect emulator
Call CreateDirectConnectGatewayAssociation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
