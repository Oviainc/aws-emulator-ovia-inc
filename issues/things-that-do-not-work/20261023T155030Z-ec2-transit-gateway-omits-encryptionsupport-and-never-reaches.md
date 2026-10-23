Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-tgw-lifecycle-scoped-describes-and-propagation-error
Issue-Step: teardown#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-tgw-lifecycle-scoped-describes-and-propagation-error ec2-transit-gateway-create-with-cidr-blocks
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeTransitGateways|2xx->2xx|$
Issue-Key: ec2|DescribeTransitGateways|2xx->2xx|$.DescribeTransitGatewaysResponse.transitGatewaySet.item.state
Issue-Key: ec2|DescribeTransitGateways|2xx->2xx|$.DescribeTransitGatewaysResponse.transitGatewaySet.item.options.encryptionSupport
Issue-Key: ec2|DescribeTransitGateways|2xx->2xx|$headers.content-type
Issue-Key: ec2|CreateTransitGateway|2xx->2xx|$.CreateTransitGatewayResponse.transitGateway.options.encryptionSupport
Issue-Key: ec2|CreateTransitGateway|2xx->2xx|$headers.content-type
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 transit gateway omits encryptionSupport and never reaches deleted

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTransitGateways

## Description
Three causes on the transit-gateway surface: the options block is serialized without encryptionSupport on both the create and the describe path, and a deleted transit gateway never leaves state 'available'.

Cause 1 of 3 — DescribeTransitGateways after DeleteTransitGateway never reports state 'deleted'.
AWS: once DeleteTransitGateway is accepted the transit gateway transitions 'deleting' -> 'deleted' and DescribeTransitGateways reports it as 'deleted'. Smithy: no state machine is modeled, so this is recorded behavior only — the fixture's recorded teardown polls DescribeTransitGateways until state is 'deleted' and gets there.
Emulator: the transit gateway stays state 'available' indefinitely; the poll reports "not in ['deleted'] after 7 attempt(s)" and caps at 60s.
Trace: ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#15.

Cause 2 of 3 — DescribeTransitGateways omits options.encryptionSupport.
AWS: the options block carries encryptionSupport {encryptionState 'disabled'} alongside the other option members (amazonSideAsn, autoAcceptSharedAttachments, defaultRouteTableAssociation, defaultRouteTablePropagation, vpnEcmpSupport, dnsSupport, securityGroupReferencingSupport, multicastSupport), which the emulator does render. Recorded behavior only.
Emulator: encryptionSupport is absent from options.
Trace: ec2-tgw-lifecycle-scoped-describes-and-propagation-error#task#2 (also ec2-transit-gateway-create-with-cidr-blocks#task#2).

Cause 3 of 3 — CreateTransitGateway's own response omits options.encryptionSupport.
AWS: CreateTransitGatewayResponse.transitGateway.options contains encryptionSupport {encryptionState 'disabled'}.
Emulator: absent.
Trace: ec2-transit-gateway-create-with-cidr-blocks#task#1.

Why: a client that deletes a transit gateway and waits for 'deleted' before releasing its dependencies (attachments, route tables, the VPCs behind them) never unblocks, and a resource the caller believes it deleted keeps reporting 'available' — so cleanup and any delete-then-recreate flow stall. Separately, a client that reads options.encryptionSupport.encryptionState to decide whether transit-gateway encryption is on gets no member at all rather than 'disabled', which an SDK surfaces as null and a policy check reads as 'unknown'.

Fix: the ec2 service crate's transit-gateway state (DeleteTransitGateway's handler must drive the state transition the describe path reads) and the TransitGatewayOptions serializer shared by CreateTransitGateway and DescribeTransitGateways (it must emit encryptionSupport).

Related open issues: 20261023T102725Z-ec2-describe-reads-return-wrong-page-contents-no.md and 20261023T102725Z-ec2-response-bodies-omit-modeled-members-and-report.md look similar but carry none of these three replay keys (owned_by: [] on all three clusters), so they are filed here. Note DeleteTransitGateway itself is a separate unimplemented scaffold already tracked by 20261023T102725Z-ec2-deletetransitgateway-stub.md; cause 1 is the describe-side state that must settle once it is implemented.

Traces: ec2-tgw-lifecycle-scoped-describes-and-propagation-error#teardown#15 (representative), ec2-tgw-lifecycle-scoped-describes-and-propagation-error#task#2, ec2-transit-gateway-create-with-cidr-blocks#task#1.

## Evidence
Replay of simparity-wire-aws-v1, 3 divergence clusters, all status 200 expected and returned.

Cause 1 — divergence_key 'service=ec2|DescribeTransitGateways|status=200->200|keys=$,$.DescribeTransitGatewaysResponse.transitGatewaySet.item.state', fixture ec2-tgw-lifecycle-scoped-describes-and-propagation-error step teardown#15: detail "$: poll capped at 60s" and "state: poll: not in ['deleted'] after 7 attempt(s) (expected None, got 'available')"; the returned item still shows state 'available'.

Cause 2 — divergence_key 'service=ec2|DescribeTransitGateways|status=200->200|keys=$.DescribeTransitGatewaysResponse.transitGatewaySet.item.options.encryptionSupport,$headers.content-type', fixture ec2-tgw-lifecycle-scoped-describes-and-propagation-error step task#2 (affected also ec2-transit-gateway-create-with-cidr-blocks#task#2): "options.encryptionSupport: missing (expected {'encryptionState': 'disabled'}, got None)".

Cause 3 — divergence_key 'service=ec2|CreateTransitGateway|status=200->200|keys=$.CreateTransitGatewayResponse.transitGateway.options.encryptionSupport,$headers.content-type', fixture ec2-transit-gateway-create-with-cidr-blocks step task#1: "transitGateway.options.encryptionSupport: missing (expected {'encryptionState': 'disabled'}, got None)".

(The tgw-/tgw-rtb- ids, ARNs, requestIds and creationTime in the recorded bodies are environment-specific and are not asserted.)

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-transit-gateway-create-with-cidr-blocks, step task#1: CreateTransitGateway's response options block has no encryptionSupport child.
3. Replay fixture ec2-tgw-lifecycle-scoped-describes-and-propagation-error, step task#2: DescribeTransitGateways shows the same omission.
4. Let that fixture run to teardown#15: after the delete, DescribeTransitGateways keeps reporting state 'available' and the poll for 'deleted' caps at 60s.
