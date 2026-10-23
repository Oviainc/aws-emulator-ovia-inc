Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-tgw-lifecycle-scoped-describes-and-propagation-error
Issue-Step: teardown#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-tgw-lifecycle-scoped-describes-and-propagation-error ec2-transit-gateway-create-with-cidr-blocks
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteTransitGateway
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# Implement ec2 DeleteTransitGateway: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTransitGateway

## Description
`DeleteTransitGateway` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_transit_gateway.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `DeleteTransitGateway` call, so each one unblocks only when `DeleteTransitGateway` works: `ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#14`, `ec2-transit-gateway-create-with-cidr-blocks#teardown#3`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_transit_gateway.rs`. Replaying the corpus, the emulator refused the `DeleteTransitGateway` call: status: expected 200, got 501
$.DeleteTransitGatewayResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'transitGateway': {'creationTime': '<shaped-not-asserted>', 'description': 'sim2real-probe-ec2-tgw-e12f845b', 'options': {'amazonSideAsn': '64512', 'associationDefaultRouteTableId': 'tgw-rtb-0000000000000001d', 'autoAcceptSharedAttachments': 'disable', 'defaultRouteTableAssociation': 'enable', 'defaultRouteTablePropagation': 'disable', 'dnsSupport': 'enable', 'encryptionSupport': {'encryptionState': 'disabled'}, 'multicastSupport': 'disable', 'securityGroupReferencingSupport': 'disable', 'vpnEcmpSupport': 'enable'}, 'ownerId': '000000000000', 'state': 'deleting', 'transitGatewayArn': 'arn:aws:ec2:us-west-2:000000000000:transit-gateway/tgw-0000000000000001c', 'transitGatewayId': 'tgw-0000000000000001c'}}, got None)
$headers.content-type: value_mismatch (expected 'text/xml;charset=UTF-8', got 'text/xml')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteTransitGateway and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
