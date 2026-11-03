Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-tgwc-traj-attachment-lookup-and-filter-mistakes
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-tgwc-traj-attachment-lookup-and-filter-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:GetTransitGatewayAttachmentPropagations
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# Implement ec2 GetTransitGatewayAttachmentPropagations: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTransitGatewayAttachmentPropagations

## Description
`GetTransitGatewayAttachmentPropagations` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_transit_gateway_attachment_propagations.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetTransitGatewayAttachmentPropagations` call, so each one unblocks only when `GetTransitGatewayAttachmentPropagations` works: `ec2-tgwc-traj-attachment-lookup-and-filter-mistakes#task#7`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/get_transit_gateway_attachment_propagations.rs`. Replaying the corpus, the emulator refused the `GetTransitGatewayAttachmentPropagations` call: status: expected 400, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'InvalidTransitGatewayAttachmentID.NotFound', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected 'Transit Gateway Attachment tgw-attach-0a1b2c3d4e5f60718 was deleted or does not exist.', got 'operation GetTransitGatewayAttachmentPropagations is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call GetTransitGatewayAttachmentPropagations and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
