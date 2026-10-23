Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-accept-vpc-peering-connection-lifecycle
Issue-Step: setup#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-accept-vpc-peering-connection-lifecycle ec2-client-vpn-endpoint-mutual-cert-auth-attempt ec2-create-vpc-availability-and-delete-lifecycle ec2-local-gateway-vpc-association-route-table-not-found ec2-create-security-group-returns-group-id ec2-security-group-rule-lifecycle ec2-network-insights-path-analysis-lifecycle ec2-network-interface-create-and-delete ec2-t12-post-delete-not-found-probes ec2-subnet-route-table-lifecycle ec2-vpc-attributes-and-account-settings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateTags|2xx->2xx|$.CreateTagsResponse.return
Issue-Key: ec2|CreateTags|2xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteVpc|2xx->2xx|$.DeleteVpcResponse.return
Issue-Key: ec2|DeleteVpc|2xx->2xx|$headers.content-type
Issue-Key: ec2|CreateSecurityGroup|2xx->2xx|$.CreateSecurityGroupResponse.return
Issue-Key: ec2|CreateSecurityGroup|2xx->2xx|$headers.content-type
Issue-Key: ec2|DeleteSubnet|2xx->2xx|$.DeleteSubnetResponse.return
Issue-Key: ec2|DeleteSubnet|2xx->2xx|$headers.content-type
Issue-Key: ec2|ModifySubnetAttribute|2xx->2xx|$.ModifySubnetAttributeResponse.return
Issue-Key: ec2|ModifySubnetAttribute|2xx->2xx|$headers.content-type
Issue-Key: ec2|ModifyVpcAttribute|2xx->2xx|$.ModifyVpcAttributeResponse.return
Issue-Key: ec2|ModifyVpcAttribute|2xx->2xx|$headers.content-type
Issue-Commit: 91a1d12adf13e91eb189076c1b2883c7fc3c7ce7

# EC2 mutation responses omit the ec2Query return element

> _Note: this issue was identified against the code base at commit `91a1d12adf13e91eb189076c1b2883c7fc3c7ce7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
Six mutation responses return HTTP 200 with a well-formed <OpResponse> that is missing its <return>true</return> child. All six must be closed before submitting. Background: these operations model no output members (Smithy com-amazonaws-ec2-amazonec2: CreateTags and DeleteVpc both have output_shape Unit), so the <return> boolean is part of the ec2Query envelope AWS writes rather than a modeled member - recorded behavior only, and every recording in this corpus carries it. Its value in the recordings is always 'true' on success.

Cause 1 of 6: CreateTags returns <CreateTagsResponse> with requestId only.
AWS: <CreateTagsResponse><requestId/><return>true</return></CreateTagsResponse>.
Emulator: the return element is absent.
Why: SDK and script callers that read the boolean to confirm the mutation (every `aws ec2 create-tags` style check, and generated clients that map the envelope's return onto a success flag) read a null and treat a successful call as indeterminate.
Fix: the ec2Query serializer for Unit-output mutations (shared), or the CreateTags handler's response builder.
Traces: ec2-accept-vpc-peering-connection-lifecycle#setup#2 (cluster also covers setup#4).

Cause 2 of 6: DeleteVpc returns <DeleteVpcResponse> without return.
AWS: return 'true'. Smithy: DeleteVpc output_shape Unit.
Emulator: absent. Cluster covers 5 steps across 4 fixtures.
Fix: as cause 1, DeleteVpc response builder.
Traces: ec2-accept-vpc-peering-connection-lifecycle#teardown#9.

Cause 3 of 6: CreateSecurityGroup returns groupId but no return element.
AWS: the response carries both groupId and return 'true'.
Emulator: return absent.
Fix: CreateSecurityGroup response builder.
Traces: ec2-create-security-group-returns-group-id#task#2 (cluster also covers ec2-security-group-rule-lifecycle#setup#2).

Cause 4 of 6: DeleteSubnet returns <DeleteSubnetResponse> without return.
Emulator: absent. Cluster covers 3 steps.
Fix: DeleteSubnet response builder.
Traces: ec2-network-insights-path-analysis-lifecycle#teardown#14.

Cause 5 of 6: ModifySubnetAttribute returns <ModifySubnetAttributeResponse> without return.
Fix: ModifySubnetAttribute response builder.
Traces: ec2-subnet-route-table-lifecycle#task#6.

Cause 6 of 6: ModifyVpcAttribute returns <ModifyVpcAttributeResponse> without return.
Fix: ModifyVpcAttribute response builder. Cluster covers 2 steps.
Traces: ec2-vpc-attributes-and-account-settings#task#3.

All six are the same shape of omission, so a fix in the shared ec2Query envelope writer very likely closes all of them at once - but each has its own replay key and each must be re-replayed.

## Evidence
Six clusters, all status 200 -> 200, each whose only body divergence is '$.<Op>Response.return: missing (expected true, got None)': CreateTagsResponse.return (2 steps), DeleteVpcResponse.return (5 steps), CreateSecurityGroupResponse.return (2 steps), DeleteSubnetResponse.return (3 steps), ModifySubnetAttributeResponse.return (1 step), ModifyVpcAttributeResponse.return (2 steps).

## Steps to Reproduce
Replay the fixture steps named under Traces and read the raw XML. Minimal repros: ec2:CreateTags on an existing resource; ec2:DeleteVpc on an empty VPC; ec2:CreateSecurityGroup; ec2:DeleteSubnet on an empty subnet; ec2:ModifySubnetAttribute MapPublicIpOnLaunch={'Value':True}; ec2:ModifyVpcAttribute EnableDnsHostnames={'Value':True}. In each case the <...Response> body has no <return> child.
