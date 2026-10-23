Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-allowed-images-settings-get-contract
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-allowed-images-settings-get-contract ec2-eip-address-association-not-found ec2-networking-association-error-paths-vpc-and-network ec2-local-gateway-vif-group-association-describe-filter-and-dry-run ec2-run-instances-validation-and-diagnostic-interrupt ec2-spot-placement-scores-queries ec2-t12-ebs-volume-attach-detach ec2-tgw-lifecycle-scoped-describes-and-propagation-error
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|GetAllowedImagesSettings|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|GetAllowedImagesSettings|4xx->4xx|$headers.content-type
Issue-Key: ec2|AssociateAddress|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|AssociateAddress|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations|4xx->4xx|$headers.content-type
Issue-Key: ec2|RunInstances|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|RunInstances|4xx->4xx|$headers.content-type
Issue-Key: ec2|GetSpotPlacementScores|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|GetSpotPlacementScores|4xx->4xx|$headers.content-type
Issue-Key: ec2|DescribeVolumesModifications|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DescribeVolumesModifications|4xx->4xx|$headers.content-type
Issue-Key: ec2|DisableTransitGatewayRouteTablePropagation|4xx->4xx|$.Response.RequestID
Issue-Key: ec2|DisableTransitGatewayRouteTablePropagation|4xx->4xx|$headers.content-type
Issue-Commit: 6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5

# EC2 error envelope drops RequestID across seven more operations

> _Note: this issue was identified against the code base at commit `6d7833292dd9d4a45d8e52e2aa7ee7e5a822f7f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAllowedImagesSettings

## Description
ONE cause, replayed on seven separate operations. The emulator's ec2Query ERROR envelope is serialized without the <RequestID> element.

Cause 1 of 1: the <Response> error envelope has <Errors><Error>...</Error></Errors> but no sibling <RequestID>.

AWS: the EC2 Smithy model binds no explicit error shapes to these operations (smithy_query query=operation reports "errors": [] for DescribeInstances, DetachVolume and the rest), so the envelope shape is recorded behavior only. Every recorded ec2Query error in the corpus is <Response><Errors><Error><Code>..</Code><Message>..</Message></Error></Errors><RequestID>..</RequestID></Response>. The requirement is that the element be PRESENT and carry a request id; the specific uuid in each recording is environment-specific and is not a requirement (two of the seven recordings assert it as '<shaped-not-asserted>').

Emulator: the element is absent entirely — the replay reports '$.Response.RequestID: missing (expected ..., got None)'. The HTTP status and the error Code match the recording on all seven steps, including the DryRun 412 responses.

Why: a client cannot correlate a failed EC2 call with anything. The AWS SDKs populate ResponseMetadata.RequestId / the RequestId on the raised error from this element; with it absent, SDK error objects carry a null request id, structured logging of failures loses its only correlation key, and any retry/support flow that echoes the request id back has nothing to echo.

Fix: the ec2Query ERROR serializer in the ec2 service crate — the envelope writer must emit <RequestID> alongside <Errors>, from the same per-request id the success path already uses (success bodies in this corpus do carry requestId).

Related open issues: 20261023T102725Z-ec2-ec2query-error-envelope-omits-the-requestid-element.md and 20261023T135552Z-cloudtrail-ec2-ec2-error-responses-carry-no-request-id-so.md describe the same fault, but neither carries the replay key for any of the seven steps below (the replay reports owned_by: [] for all of them), so they are filed here.

Traces: ec2-allowed-images-settings-get-contract#task#2 (GetAllowedImagesSettings, 412 DryRun); ec2-eip-address-association-not-found#task#1 (AssociateAddress, 400); ec2-local-gateway-vif-group-association-describe-filter-and-dry-run#task#3 (DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations, 412 DryRun); ec2-run-instances-validation-and-diagnostic-interrupt#task#4 (RunInstances, 412 DryRun); ec2-spot-placement-scores-queries#task#3 (GetSpotPlacementScores, 412 DryRun); ec2-t12-ebs-volume-attach-detach#task#11 (DescribeVolumesModifications, 400); ec2-tgw-lifecycle-scoped-describes-and-propagation-error#task#13 (DisableTransitGatewayRouteTablePropagation, 400).

## Evidence
Replay of simparity-wire-aws-v1, 7 divergence clusters whose only body-level diverging key is $.Response.RequestID.

Expected (recorded AWS): error body contains a <RequestID> element.
Actual (emulator):      element absent (None).

Representative cluster: divergence_key 'service=ec2|GetAllowedImagesSettings|status=412->412|keys=$.Response.RequestID,$headers.content-type', fixture ec2-allowed-images-settings-get-contract step task#2, expected_status 412, actual_status 412, detail "$.Response.RequestID: missing (expected '9588e77d-a4b4-4226-b0fd-4c8010f31bfd', got None)".

Other members: AssociateAddress 400 (ec2-eip-address-association-not-found#task#1, also seen on ec2-networking-association-error-paths-vpc-and-network#task#1); DescribeLocalGatewayRouteTableVirtualInterfaceGroupAssociations 412; RunInstances 412; GetSpotPlacementScores 412; DescribeVolumesModifications 400; DisableTransitGatewayRouteTablePropagation 400. The recorded uuid differs per step and is an example only; the requirement is the element's presence.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus().
2. Replay fixture ec2-allowed-images-settings-get-contract, step task#2 — a GetAllowedImagesSettings call with DryRun set, which AWS answers 412.
3. Parse the XML error body: <Response><Errors><Error>..</Error></Errors></Response> with no <RequestID> child.
4. Repeat for each fixture#step in trace_ids (mix of 400 validation/not-found errors and 412 DryRun errors) — the element is missing on every one.
