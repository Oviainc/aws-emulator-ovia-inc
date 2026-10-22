Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dax.invalid-subnet-rejected dax.describe-events-invalid-duration-rejected dax.delete-nonexistent-cluster-rejected dax.max-results-out-of-range-rejected
Issue-Shape-Id: com.amazonaws.dax#AmazonDAXV3
Issue-Key: dax.invalid-subnet-rejected|_error_code#4|AssertionError|raise AssertionError(f"{context}: expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: dax.describe-events-invalid-duration-rejected|run#3|AssertionError|assert code == "InvalidParameterCombinationException", (
Issue-Key: dax.delete-nonexistent-cluster-rejected|run#5|AssertionError|assert message == "Cluster not found.", f"unexpected message: {message!r}"
Issue-Key: dax.max-results-out-of-range-rejected|run#4|AssertionError|assert below_message == "Invalid value for max records. Must be between 20 and 100", (
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# DAX rejection paths diverge: CreateSubnetGroup accepts a nonexistent subnet id, DescribeEvents answers 501, and not-found/MaxResults error messages use the wrong templates

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubnetGroup, DescribeEvents, DeleteCluster, DescribeClusters

## Description
Three distinct root causes in the DAX emulator, all found by replaying the behavioral suite against the running dax emulator (service-com-amazonaws-dax-amazondaxv3, bootstrapped and answering). The suite's other dax failures are blocked by already-filed unimplemented stubs (IAM CreateRole, EC2 CreateVpc, dax CreateParameterGroup / DescribeParameterGroups / DescribeDefaultParameters) and are NOT part of this issue.

Cause 1 of 3 — CreateSubnetGroup accepts a subnet identifier that does not exist (over-permissive accept).
AWS: CreateSubnetGroup refuses a request naming a subnet that does not exist in the account/region. Smithy (services/service-com-amazonaws-dax-amazondaxv3/smithy-model/com-amazonaws-dax-amazondaxv3.json): CreateSubnetGroup models InvalidSubnet (HTTP 400, "An invalid subnet identifier was specified") and SubnetNotAllowedFault (HTTP 400) for exactly this class of input; the recorded AWS response for a bogus id is HTTP 400 with error code InvalidParameterValueException and a message of the form "Some input subnets in :[<subnet-ids>] are invalid." (recorded behavior for the code/message; the model additionally offers InvalidSubnet). After the rejection, DescribeSubnetGroups on that name must fail with SubnetGroupNotFoundFault — i.e. no side effect.
Emulator: the create SUCCEEDS with HTTP 200 and persists a subnet group, e.g. {'SubnetGroup': {'SubnetGroupName': 'sim2real-dax-badsng-...', 'VpcId': 'vpc-f13c813a', 'Subnets': [{'SubnetIdentifier': 'subnet-0000000000deadbee', 'SubnetAvailabilityZone': 'us-west-2b', ...}]}}. The handler only validates that SubnetIds is non-empty and within the per-group quota and then DERIVES the VpcId and each AZ from the identifier text, so any syntactically arbitrary string is accepted as a subnet.
Why: a client that relies on DAX refusing an unusable subnet group gets a 200 and a subnet group pointing at subnets that do not exist; the failure surfaces later (or never), and negative-path code that expects a 400 rejection never runs. An over-permissive accept also silently creates state the test/cleanup path does not expect.
Fix: CreateSubnetGroup's input validation in the operation handler — reject identifiers that cannot name a real subnet (at minimum the subnet-<8|17 hex> form, and ideally any id not known to the emulator's network model) with the recorded 400/InvalidParameterValueException (or the modeled InvalidSubnet) and persist nothing.
Traces: dax.invalid-subnet-rejected. Handler: services/service-com-amazonaws-dax-amazondaxv3/src/ops/create_subnet_group.rs lines 162-217.

Cause 2 of 3 — DescribeEvents is an unimplemented scaffold stub, so both its rejection and its success path are wrong.
AWS: DescribeEvents returns an Events list (HTTP 200) and, for a Duration beyond the documented 14-day window, rejects with HTTP 400 InvalidParameterCombinationException. Smithy: DescribeEvents input Duration (IntegerOptional), MaxResults documented "must be between 20 and 100"; modeled errors InvalidParameterCombinationException (400), InvalidParameterValueException (400), ServiceLinkedRoleNotFoundFault (400). Recorded message example for the rejection: "Events occurring more than 14 days in the past are not available."
Emulator: every DescribeEvents call answers HTTP 501 with error code NotImplemented ("operation DescribeEvents is scaffolded but not implemented in this emulator"), both for Duration=20161 (expected 400 InvalidParameterCombinationException) and for the in-range positive control Duration=60 (expected 200 with an Events key).
Why: a client cannot read DAX events at all and sees a 501 — not a modeled, retry/branch-able client error — where AWS returns either a 200 payload or a 400 with a modeled error code.
Fix: implement the DescribeEvents handler (reject an out-of-window Duration with the modeled InvalidParameterCombinationException, otherwise return the Events list) and remove the SCAFFOLDED marker.
Traces: dax.describe-events-invalid-duration-rejected. Handler: services/service-com-amazonaws-dax-amazondaxv3/src/ops/describe_events.rs lines 158-176 (returns Scaffolded::not_implemented).

Cause 3 of 3 — error-message templates for DAX rejections do not follow AWS's per-fault templates (HTTP status and error code are correct; only the message text diverges).
AWS: recorded behavior only — DAX uses a fixed message template per fault, and the template differs per fault in whether it echoes the request. ClusterNotFoundFault carries the generic, identifier-free message "Cluster not found." (and ParameterGroupNotFoundFault "Parameter Group not found."), while SubnetGroupNotFoundFault does echo the name ("Subnet group <name> not found."). The DescribeClusters MaxResults range rejection is HTTP 400 InvalidParameterValueException with the template "Invalid value for max records. Must be between 20 and 100" (the model carries no @range on MaxResults; the 20-100 range is documented on the member).
Emulator: the codes match, the messages are rewritten and echo the request. DeleteCluster on an absent cluster returns ClusterNotFoundFault with message 'Cluster sim2real-dax-missing-5516309b87dc does not exist' instead of the generic "Cluster not found."; DescribeClusters(MaxResults=5) returns InvalidParameterValueException with message 'MaxResults must be between 20 and 100, got 5' instead of "Invalid value for max records. Must be between 20 and 100".
Why: clients and tooling that match DAX's documented/recorded message text (and golden-output tests) diverge, and the not-found message now leaks the requested identifier where AWS deliberately returns a generic one.
Fix: the error-construction sites in the DeleteCluster and DescribeClusters handlers (and any shared not-found helper in the dax crate's state/error module) should emit the per-fault AWS template rather than an ad-hoc formatted string.
Traces: dax.delete-nonexistent-cluster-rejected, dax.max-results-out-of-range-rejected.

## Evidence
Suite: 0/21 passed for dax; 4 mismatches, 17 errors (the errors are the already-filed stub clusters).

1) dax.invalid-subnet-rejected [mismatch]
   Expected: ClientError, code InvalidParameterValueException, message "Some input subnets in :[subnet-0000000000deadbee] are invalid."
   Actual:   HTTP 200 — {'SubnetGroup': {'SubnetGroupName': 'sim2real-dax-badsng-be1fbcffba99', 'VpcId': 'vpc-f13c813a', 'Subnets': [{'SubnetIdentifier': 'subnet-0000000000deadbee', 'SubnetAvailabilityZone': 'us-west-2b', 'SupportedNetworkTypes': ['ipv4']}], 'SupportedNetworkTypes': ['ipv4']}}
   AssertionError: create_subnet_group with a nonexistent subnet id: expected a ClientError, but the call SUCCEEDED

2) dax.describe-events-invalid-duration-rejected [mismatch]
   Expected: describe_events(Duration=20161) -> 400 InvalidParameterCombinationException; describe_events(Duration=60) -> 200 with 'Events'
   Actual:   AssertionError: expected InvalidParameterCombinationException, got 'NotImplemented' (HTTP 501, "operation DescribeEvents is scaffolded but not implemented in this emulator")

3) dax.delete-nonexistent-cluster-rejected [mismatch]
   Expected: code ClusterNotFoundFault, message "Cluster not found."
   Actual:   code ClusterNotFoundFault (correct), message 'Cluster sim2real-dax-missing-5516309b87dc does not exist'

4) dax.max-results-out-of-range-rejected [mismatch]
   Expected: code InvalidParameterValueException, message "Invalid value for max records. Must be between 20 and 100"
   Actual:   code InvalidParameterValueException (correct), message 'MaxResults must be between 20 and 100, got 5'

## Steps to Reproduce
Boot the emulator and point a botocore dax client at it, then:

Cause 1: dax.create_subnet_group(SubnetGroupName='x-badsng', SubnetIds=['subnet-0000000000deadbee']) -> observe HTTP 200 and a persisted subnet group; AWS answers 400 InvalidParameterValueException and leaves nothing behind (dax.describe_subnet_groups(SubnetGroupNames=['x-badsng']) must raise SubnetGroupNotFoundFault).

Cause 2: dax.describe_events(Duration=20161) -> observe 501 NotImplemented; AWS answers 400 InvalidParameterCombinationException. Then dax.describe_events(Duration=60) -> observe 501; AWS answers 200 with an 'Events' key.

Cause 3: dax.delete_cluster(ClusterName='never-created') -> inspect Error.Message (emulator echoes the name; AWS returns "Cluster not found."). dax.describe_clusters(MaxResults=5) and dax.describe_clusters(MaxResults=101) -> inspect Error.Message (emulator: 'MaxResults must be between 20 and 100, got 5'; AWS: "Invalid value for max records. Must be between 20 and 100").

Or replay the checks directly: dax.invalid-subnet-rejected, dax.describe-events-invalid-duration-rejected, dax.delete-nonexistent-cluster-rejected, dax.max-results-out-of-range-rejected.
