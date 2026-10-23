Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-error-paths-invalid-table-entry-parents
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-error-paths-invalid-table-entry-parents
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:CreateTransitGatewayPrefixListReference
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 CreateTransitGatewayPrefixListReference: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransitGatewayPrefixListReference

## Description
`CreateTransitGatewayPrefixListReference` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_transit_gateway_prefix_list_reference.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateTransitGatewayPrefixListReference` call, so each one unblocks only when `CreateTransitGatewayPrefixListReference` works: `ec2-error-paths-invalid-table-entry-parents#task#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_transit_gateway_prefix_list_reference.rs`. Replaying the corpus, the emulator refused the `CreateTransitGatewayPrefixListReference` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidRouteTableId.Malformed', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected 'Invalid Transit Gateway Route Table id tgw-rtb-0123456789abcde.', got 'operation CreateTransitGatewayPrefixListReference is scaffolded but not implemented in this emulator')
$.Response.RequestID: missing (expected 'caff0c81-1459-4c20-b6e4-42b3fe289e5e', got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateTransitGatewayPrefixListReference and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
