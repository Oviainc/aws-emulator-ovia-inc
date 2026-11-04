Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-application-status-check-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-application-status-check-lifecycle ec2-internet-gateway-attachment-errors
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DeleteApplicationStatusCheck|2xx->2xx|$.DeleteApplicationStatusCheckResponse.applicationStatusCheck.deviceIndex
Issue-Key: ec2|DeleteApplicationStatusCheck|2xx->2xx|$.DeleteApplicationStatusCheckResponse.applicationStatusCheck.ipScope
Issue-Key: ec2|DeleteApplicationStatusCheck|2xx->2xx|$.DeleteApplicationStatusCheckResponse.applicationStatusCheck.ipVersion
Issue-Key: ec2|CreateInternetGateway|2xx->2xx|$.CreateInternetGatewayResponse.internetGateway.association
Issue-Commit: 44d77109d3457b765fae885a8f4ecd8776c4625f

# EC2 success responses omit members AWS returns for status-check and gateway reads

> _Note: this issue was identified against the code base at commit `44d77109d3457b765fae885a8f4ecd8776c4625f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteApplicationStatusCheck

## Description
Two net-new wire divergences with one shared fault: the emulator's ec2 response serializers drop members that AWS puts on the wire, so a 200 answer is shape-incomplete. This issue bundles 2 causes; both must be closed.

Cause 1 of 2: DeleteApplicationStatusCheck omits the deviceIndex, ipScope and ipVersion members of the returned status check.
AWS: Smithy com.amazonaws.ec2#DeleteApplicationStatusCheck has output DeleteApplicationStatusCheckResult.ApplicationStatusCheck (xmlName applicationStatusCheck) targeting ApplicationStatusCheckResponseObject, which models DeviceIndex (xmlName deviceIndex, Integer), IpScope (xmlName ipScope, enum IpScopeEnum values [private]) and IpVersion (xmlName ipVersion, enum IpVersionEnum values [ipv4, ipv6]). The recorded response carries all three, echoing the configuration the check was created with (deviceIndex the device index of the checked interface, ipScope the configured scope, ipVersion the configured address family).
Emulator: the applicationStatusCheck element is serialized without deviceIndex, ipScope and ipVersion at all (the other members are present and matched).
Why: a client reading back the deleted check cannot see which interface/address family it covered, and an SDK that treats a modeled enum member as always-present on this response gets a null where AWS always returns a value, so delete-time reconciliation of status-check configuration is impossible.
Fix: the DeleteApplicationStatusCheck response projection/serializer in the ec2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_application_status_check.rs and the shared ApplicationStatusCheckResponseObject projection) must carry the stored deviceIndex, ipScope and ipVersion through from status-check state instead of dropping them.
Traces: ec2-application-status-check-lifecycle#task#3.

Cause 2 of 2: CreateInternetGateway omits the association element on the created gateway.
AWS: recorded behavior only - the on-disk model's com.amazonaws.ec2#InternetGateway declares only Attachments (attachmentSet), InternetGatewayId, OwnerId and Tags, but the recording shows AWS serializing an additional, empty association element inside internetGateway on the CreateInternetGateway response (expected value '', i.e. the element is present and empty for a freshly created, unassociated gateway).
Emulator: serializes internetGateway with no association element at all.
Why: a client (or a recorded-contract consumer) that reads internetGateway.association to decide whether the new gateway is already associated finds the member absent rather than present-and-empty, which is a different answer from AWS for every newly created gateway.
Fix: the CreateInternetGateway response serializer in the ec2 crate (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs, with the internet-gateway projection shared with DescribeInternetGateways) must emit the empty association element for a gateway with no association.
Traces: ec2-internet-gateway-attachment-errors#setup#2.

## Evidence
Cause 1 - fixture ec2-application-status-check-lifecycle step task#3, DeleteApplicationStatusCheck, status 200 vs 200: $.DeleteApplicationStatusCheckResponse.applicationStatusCheck.deviceIndex missing (expected '0', got None); .ipScope missing (expected 'private', got None); .ipVersion missing (expected 'ipv4', got None). The recorded values are the ones this request configured - the requirement is that the members are present and echo stored state, not those literals.
Cause 2 - fixture ec2-internet-gateway-attachment-errors step setup#2, CreateInternetGateway, status 200 vs 200: $.CreateInternetGatewayResponse.internetGateway.association missing (expected '', got None).
Model evidence: smithy_query closure DeleteApplicationStatusCheck and shape InternetGateway on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json - ApplicationStatusCheckResponseObject models deviceIndex/ipScope/ipVersion; InternetGateway does not model association, so cause 2 rests on the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture ec2-application-status-check-lifecycle step task#3 and diff the applicationStatusCheck element against the recorded response (deviceIndex, ipScope, ipVersion).
3. Replay fixture ec2-internet-gateway-attachment-errors step setup#2 and diff internetGateway against the recorded response (association).
