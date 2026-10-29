Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-subnet-cidr-overlap
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-subnet-cidr-overlap ec2-subnet-route-table-lifecycle ec2-conflict-volume-attachment ec2-t12-ebs-volume-attach-detach ec2-create-snapshots-invalid-instance-error ec2-instance-dependent-ops-error-paths ec2-custom-ip-provisioning-and-legacy-network-feature-discovery ec2-customer-gateway-lifecycle ec2-describe-network-insights-analyses-contract
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateSubnet|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateVpc|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|AttachVolume|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteVolume|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateSnapshots|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCoipPools|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateCustomerGateway|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeNetworkInsightsAnalyses|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: 14bf84ae7f57c35d094a91bae03d265103651cef

# EC2 refusal messages reword AWS text across eight operations

> _Note: this issue was identified against the code base at commit `14bf84ae7f57c35d094a91bae03d265103651cef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubnet

## Description
Eight EC2 refusal paths return the right HTTP status (400) and, in these clusters, the right error Code, but the Error.Message is emulator-authored prose rather than AWS's wording: the emulator adds detail AWS omits, drops values AWS includes, or orders list contents differently. An error message is part of the contract, so each of these is a separate per-operation message defect. This is NOT covered by the already-open EC2 message issues (20261029T200817Z-ec2-not-found-and-malformed-refusals-emit-generic.md, 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md, 20261029T175114Z-ec2-refusal-messages-are-emulator-authored-prose-instead.md): the replay reports these eight clusters with an EMPTY owned_by, i.e. no open issue carries their replay key. There are 8 causes; all 8 must be fixed before this issue is closed.

Cause 1 of 8 - CreateSubnet, overlapping CIDR.
AWS: recorded behavior only (ec2 query protocol models no per-case error shape). AWS answers 400 with Code InvalidSubnet.Conflict and Message fixed text "The CIDR '<cidr>' conflicts with another subnet", where <cidr> is the requested CidrBlock from the request. AWS does NOT name the conflicting subnet.
Emulator: same status and code, Message "The CIDR '<cidr>' conflicts with another subnet ('<subnet-id>')" - it appends a parenthesised subnet id taken from its own state.
Why: a client that matches or displays AWS's message text (or asserts on it in tests) sees an unexpected trailing clause, and the emulator leaks an id that AWS never returns for this error.
Fix: the CreateSubnet handler's CIDR-conflict refusal in the ec2 service crate - drop the conflicting-subnet suffix from the message it formats.
Traces: ec2-conflict-subnet-cidr-overlap#task#3.

Cause 2 of 8 - CreateVpc, invalid CIDR block size.
AWS: recorded behavior only. 400, Code InvalidParameterValue, Message fixed text "The CIDR '<cidr>' is invalid." with <cidr> taken from the request.
Emulator: appends an extra sentence - "The CIDR '<cidr>' is invalid. The allowed block size is between a /16 netmask and /28 netmask."
Why: the message no longer matches AWS for the same refusal class; clients that compare the message text diverge even though the status and code agree.
Fix: the CreateVpc handler's CIDR validation message formatting.
Traces: ec2-conflict-subnet-cidr-overlap#task#4.

Cause 3 of 8 - AttachVolume, volume and instance in different availability zones.
AWS: recorded behavior only. 400, Message fixed text "The volume '<volume-id>' is not in the same availability zone as instance '<instance-id>'", with both ids taken from the request.
Emulator: substitutes its own sentence naming both zones - "The volume '<volume-id>' is in availability zone '<az>' and the instance '<instance-id>' is in availability zone '<az>'".
Why: the refusal reason is the same but the text is entirely different, so any client surface that relays AWS's wording is wrong.
Fix: the AttachVolume handler's zone-mismatch refusal.
Traces: ec2-conflict-volume-attachment#task#10.

Cause 4 of 8 - DeleteVolume on an attached volume.
AWS: recorded behavior only. 400, Message fixed text "Volume <volume-id> is currently attached to {<instance-id>}" - AWS wraps the attached instance id list in braces.
Emulator: emits the id without the braces - "Volume <volume-id> is currently attached to <instance-id>".
Why: the braces are part of AWS's fixed text (it is a set rendering), so message comparison and any parser keyed on it fails.
Fix: the DeleteVolume handler's in-use refusal message.
Traces: ec2-conflict-volume-attachment#task#11.

Cause 5 of 8 - CreateSnapshots with an unresolvable instance id.
AWS: recorded behavior only. 400 with Message fixed text "Unable to unobfuscate ID." - AWS does not echo the offending id for this refusal.
Emulator: answers 'Invalid id: "<instance-id>"', its generic id-parse prose, echoing the request value.
Why: the client gets emulator-internal phrasing instead of the AWS message for this class, and a value AWS withholds.
Fix: the CreateSnapshots handler's instance-id resolution refusal.
Traces: ec2-create-snapshots-invalid-instance-error#task#1.

Cause 6 of 8 - DescribeCoipPools with a syntactically wrong pool id.
AWS: recorded behavior only. 400, Message fixed text "The pool ID '<pool-id>' is invalid." with the id from the request.
Emulator: answers its generic id-parse prose - 'Invalid id: "<pool-id>". A customer-owned address pool id is \'ipv4pool-coip-\' followed by 8 or 17 hexadecimal digits', which both rewords the message and adds a format lecture AWS does not send.
Why: same class, different text; clients relying on AWS's message see emulator internals.
Fix: the DescribeCoipPools handler's pool-id validation message (it is falling through to the shared id-parse formatter).
Traces: ec2-custom-ip-provisioning-and-legacy-network-feature-discovery#task#8.

Cause 7 of 8 - CreateCustomerGateway with BgpAsn 0.
AWS: recorded behavior only. 400, Message fixed text "Value (<value>) for parameter bgpAsn is invalid. Invalid Format." where <value> is the submitted BgpAsn.
Emulator: keeps AWS's prefix but replaces the trailing clause with a range - "Value (<value>) for parameter bgpAsn is invalid. Valid values: 1 to 2147483647.".
Why: AWS classifies this as a format problem, not a range problem; the emulator teaches clients a different rule than AWS states.
Fix: the CreateCustomerGateway handler's bgpAsn validation message.
Traces: ec2-customer-gateway-lifecycle#task#3.

Cause 8 of 8 - DescribeNetworkInsightsAnalyses with an unsupported filter name.
AWS: recorded behavior only. 400, Message fixed text "Filter name must be one of [<names>]" where <names> is AWS's documented filter list in AWS's order - the recording shows [status, path-found].
Emulator: emits the same sentence with the list sorted alphabetically - [path-found, status].
Why: the variable part of the message is a fixed, ordered list for this operation, so the emulator's re-ordering is a wording divergence a client comparing messages will hit.
Fix: the DescribeNetworkInsightsAnalyses filter-name validation - emit the filter list in the service's declared order rather than a sorted set.
Traces: ec2-describe-network-insights-analyses-contract#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 47.2%, 906 mismatched steps). All eight clusters report verdict=mismatch with expected_status == actual_status == 400, stub_path empty and owned_by empty.

1) ec2-conflict-subnet-cidr-overlap#task#3 CreateSubnet: $.Response.Errors.Error.Message expected "The CIDR '10.83.1.128/25' conflicts with another subnet", got "The CIDR '10.83.1.128/25' conflicts with another subnet ('subnet-00000000000000029')". (cluster also covers ec2-subnet-route-table-lifecycle#task#14)
2) ec2-conflict-subnet-cidr-overlap#task#4 CreateVpc: expected "The CIDR '10.0.0.0/8' is invalid.", got "The CIDR '10.0.0.0/8' is invalid. The allowed block size is between a /16 netmask and /28 netmask."
3) ec2-conflict-volume-attachment#task#10 AttachVolume: expected "The volume 'vol-0000000000000002d' is not in the same availability zone as instance 'i-00000000000000001'", got "The volume 'vol-0000000000000002d' is in availability zone 'us-west-2b' and the instance 'i-00000000000000001' is in availability zone 'us-west-2a'".
4) ec2-conflict-volume-attachment#task#11 DeleteVolume: expected 'Volume vol-0000000000000002c is currently attached to {i-00000000000000001}', got 'Volume vol-0000000000000002c is currently attached to i-00000000000000001'.
5) ec2-create-snapshots-invalid-instance-error#task#1 CreateSnapshots: expected 'Unable to unobfuscate ID.', got 'Invalid id: "i-00000000000000000"'.
6) ec2-custom-ip-provisioning-and-legacy-network-feature-discovery#task#8 DescribeCoipPools: expected "The pool ID 'coip-pool-00000000000000000' is invalid.", got 'Invalid id: "coip-pool-00000000000000000". A customer-owned address pool id is \'ipv4pool-coip-\' followed by 8 or 17 hexadecimal digits'.
7) ec2-customer-gateway-lifecycle#task#3 CreateCustomerGateway: expected 'Value (0) for parameter bgpAsn is invalid. Invalid Format.', got 'Value (0) for parameter bgpAsn is invalid. Valid values: 1 to 2147483647.'.
8) ec2-describe-network-insights-analyses-contract#task#3 DescribeNetworkInsightsAnalyses: expected 'Filter name must be one of [status, path-found]', got 'Filter name must be one of [path-found, status]'.

The quoted ids, CIDRs and instance types above are the values the recorded requests carried; the requirement is the message template (fixed text plus request/state-derived values), not those literals.

## Steps to Reproduce
1. Boot the emulator built from this checkout.
2. Replay trace set simparity-wire-aws-v1.
3. Inspect fixture ec2-conflict-subnet-cidr-overlap step task#3 (and the seven fixture#step bindings listed in trace_ids): each step sends the recorded request and compares the XML error body.
4. Each step reports $.Response.Errors.Error.Message value_mismatch while status (400) and, in these clusters, Code agree.
