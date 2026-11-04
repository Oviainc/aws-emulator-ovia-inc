Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-internet-gateway-attachment-errors
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-internet-gateway-attachment-errors ec2-pst-traj-detached-igw-leaves-blackhole-default-route
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateTags|2xx->4xx|$.CreateTagsResponse
Issue-Key: ec2|CreateTags|2xx->4xx|status
Issue-Key: ec2|DeleteTags|2xx->4xx|$.DeleteTagsResponse
Issue-Key: ec2|DeleteTags|2xx->4xx|status
Issue-Key: ec2|DescribeInternetGateways|4xx->2xx|$
Issue-Key: ec2|DescribeInternetGateways|4xx->2xx|$.Response
Issue-Key: ec2|DescribeInternetGateways|4xx->2xx|status
Issue-Commit: 44d77109d3457b765fae885a8f4ecd8776c4625f

# EC2 internet-gateway ids break the tag plane and deleted gateways stay describable

> _Note: this issue was identified against the code base at commit `44d77109d3457b765fae885a8f4ecd8776c4625f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
Four net-new wire divergences, all rooted in how the ec2 emulator handles an internet-gateway (igw-) resource id: the tag plane does not accept it, and the describe path keeps serving a gateway that was deleted. This issue bundles 4 causes; all four must be closed.

Cause 1 of 4: CreateTags on an internet-gateway id is refused.
AWS: Smithy com.amazonaws.ec2#CreateTags takes Resources (ResourceIdList, @required) and Tags (TagList, @required) and has output Unit and NO modeled errors (ec2Query protocol, errors are untyped), i.e. any resource id the account owns - including an internet gateway - is tagged and the call answers HTTP 200 with <CreateTagsResponse><requestId>..</requestId><return>true</return></CreateTagsResponse>. The recorded response for this step is exactly that: status 200, body {'requestId': <shaped>, 'return': 'true'}.
Emulator: answers HTTP 400 and serves no CreateTagsResponse element at all. The replay records the status and the absent success body; it does not expose the refusal's error code, so the code is not asserted here - the requirement is the 200 + return=true success envelope for an igw- id the emulator itself minted.
Why: a client that tags an internet gateway (the normal CloudFormation/CDK/console pattern of naming an IGW) gets a hard 400 for a request AWS accepts, so every provisioning flow that tags its gateway fails and the Name tag is never readable afterwards.
Fix: the CreateTags handler's resource-id resolution in the ec2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_tags.rs and the shared tag-plane id classifier it calls) must recognize the igw- id class and attach the tag set to internet-gateway state. Related but NOT covering: open issue 20261024T062131Z-route53resolver-ec2-createtags-refuses-subnet-and-security-group-ids.md reports the same failure shape for subnet- and sg- ids; it does not carry this fixture step and names a different id class.
Traces: ec2-internet-gateway-attachment-errors#task#6.

Cause 2 of 4: DeleteTags on an internet-gateway id is refused the same way.
AWS: Smithy com.amazonaws.ec2#DeleteTags takes Resources (@required) and optional Tags, output Unit, no modeled errors; the recorded response is HTTP 200 with <DeleteTagsResponse><requestId>..</requestId><return>true</return></DeleteTagsResponse>.
Emulator: answers HTTP 400 with no DeleteTagsResponse body.
Why: tags set on a gateway can never be removed, so teardown and tag-rewrite flows fail on an operation AWS always accepts for an owned id.
Fix: same resource-id resolution in the DeleteTags handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_tags.rs); both tag-plane halves must share one id classifier so read and write cannot drift.
Traces: ec2-internet-gateway-attachment-errors#task#8.

Cause 3 of 4: after DeleteInternetGateway, DescribeInternetGateways still returns the gateway instead of refusing the id.
AWS: recorded behavior (the ec2Query model declares no errors for DescribeInternetGateways): describing an internet-gateway id that no longer exists answers HTTP 400 with an error envelope whose Errors.Error.Code is InvalidInternetGatewayID.NotFound and whose Message is the fixed sentence "The internetGateway ID '<id>' does not exist" - the fixed text is "The internetGateway ID '" ... "' does not exist" and the only variable part is the gateway id taken verbatim from the request's InternetGatewayId parameter; the envelope also carries a RequestID.
Emulator: answers HTTP 200 and serves a DescribeInternetGatewaysResponse that still contains the deleted gateway (internetGatewaySet.item with the deleted id, its ownerId and its tagSet), and keeps doing so for the whole poll window (12 attempts).
Why: a client cannot observe deletion. Waiters that poll for the NotFound refusal to confirm a gateway is gone never terminate, and an id that must be rejected is treated as live, so dependent teardown ordering silently breaks.
Fix: the delete path must tombstone/remove the gateway from state and the DescribeInternetGateways handler must raise the untyped InvalidInternetGatewayID.NotFound refusal for an explicitly named unknown id (services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_internet_gateways.rs plus the internet-gateway state store).
Traces: ec2-internet-gateway-attachment-errors#teardown#11.

Cause 4 of 4: the same describe path answers 200 for a never-existing / already-removed gateway id in a second fixture.
AWS: recorded behavior - HTTP 400, Errors.Error.Code InvalidInternetGatewayID.NotFound, Message "The internetGateway ID '<id>' does not exist" (fixed text as above, id from the request).
Emulator: answers HTTP 200 and omits the Response error envelope entirely.
Why: the not-found contract for an explicitly named internet-gateway id is unreachable, so a client that uses the refusal as an existence probe concludes a gateway exists when it does not.
Fix: same handler and state as cause 3; the id-exists check must reject unknown ids rather than returning an empty-or-stale success set.
Traces: ec2-pst-traj-detached-igw-leaves-blackhole-default-route#task#14.

## Evidence
Cause 1 - fixture ec2-internet-gateway-attachment-errors step task#6, CreateTags: expected status 200 with body {'requestId': '<shaped-not-asserted>', 'return': 'true'}; actual status 400 and $.CreateTagsResponse missing.
Cause 2 - same fixture step task#8, DeleteTags: expected status 200 with {'requestId': '<shaped-not-asserted>', 'return': 'true'}; actual status 400 and $.DeleteTagsResponse missing.
Cause 3 - same fixture step teardown#11, DescribeInternetGateways: expected status 400 with $.Response = {'Errors': {'Error': {'Code': 'InvalidInternetGatewayID.NotFound', 'Message': "The internetGateway ID '<igw-id>' does not exist"}}, 'RequestID': '<shaped-not-asserted>'}; actual status 200 with DescribeInternetGatewaysResponse.internetGatewaySet.item still carrying the deleted gateway, and the poll gave up after 12 attempts with error code None not in ['InvalidInternetGatewayID.NotFound'].
Cause 4 - fixture ec2-pst-traj-detached-igw-leaves-blackhole-default-route step task#14, DescribeInternetGateways: expected status 400 with Code 'InvalidInternetGatewayID.NotFound' and Message "The internetGateway ID '<igw-id>' does not exist"; actual status 200, $.Response missing.
Model evidence: smithy_query operation CreateTags/DeleteTags/DescribeInternetGateways on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json - CreateTags/DeleteTags output Unit (success body is requestId+return=true), no modeled errors on any of the three (ec2Query untyped errors), DescribeInternetGateways output member InternetGateways is xmlName internetGatewaySet.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture ec2-internet-gateway-attachment-errors and watch steps task#6 (CreateTags on the igw- id), task#8 (DeleteTags on the same id) and teardown#11 (DescribeInternetGateways after DeleteInternetGateway).
3. Replay fixture ec2-pst-traj-detached-igw-leaves-blackhole-default-route step task#14 (DescribeInternetGateways for a gateway id that must not resolve).
4. Compare status, Errors.Error.Code and Errors.Error.Message against the recorded AWS responses.
