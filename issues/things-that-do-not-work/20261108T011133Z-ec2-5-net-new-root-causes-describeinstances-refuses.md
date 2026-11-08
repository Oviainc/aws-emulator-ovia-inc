Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.capres-targeted-reservation-consumed-by-a-guest-that-really-boots ec2.placement-partition-number-above-the-group-count-rejected ec2.placement-spread-group-rejects-more-instances-than-one-zone-can-place ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle ec2.ec2mr-dvpc-e2e-without-default-vpc-lifecycle ec2.ec2mr-pub-e2e-public-subnets-lifecycle ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle ec2.op-contract-attachVerifiedAccessTrustProvider ec2.op-contract-modify-capacity-reservation-state-conflict
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstances|InvalidParameterValue
Issue-Key: ec2|DescribeInstances|InvalidInstanceID.Malformed
Issue-Key: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle|phase#4|_PhaseError|raise _PhaseError(
Issue-Key: ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle|phase#6|_PhaseError|raise _PhaseError(
Issue-Key: ec2.op-contract-attachVerifiedAccessTrustProvider|run#23|AssertionError|assert bogus_code_message == expected_bogus_code_message, f"expected message {expected_bogus_code_message!r}, got {bogus_code_message!r}"
Issue-Key: ec2.op-contract-modify-capacity-reservation-state-conflict|run#20|AssertionError|assert conflict_code == "InvalidStateTransition", (
Issue-Commit: f4d58d8ba640bd38bd7af13aafdf9baa78485a99

# EC2: 5 net-new root causes — DescribeInstances refuses documented placement/capacity-reservation filters, CFN-built instances report a name as their instance id, stack Outputs omit OutputValue, and two refusals carry the wrong code/message

> _Note: this issue was identified against the code base at commit `f4d58d8ba640bd38bd7af13aafdf9baa78485a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstances

## Description
Five distinct net-new root causes found by the behavioral replay, all under the suite's `### ec2` heading. Each must be closed before this issue is submitted.

=== Cause 1 of 5: DescribeInstances refuses filter names the API documents ===
AWS: Smithy: DescribeInstancesRequest.Filters documents `capacity-reservation-id` ("The ID of the Capacity Reservation into which the instance was launched"), `placement-group-name` ("The name of the placement group for the instance") and `placement-partition-number` ("The partition in which the instance is located") as valid filter names. AWS accepts them and answers 200 with the matching reservation set (empty Reservations when nothing matches). Only a name that is NOT in that documented set is refused, with InvalidParameterValue.
Emulator: refuses the documented names with HTTP 400 / error code `InvalidParameterValue` and message `The filter 'capacity-reservation-id' is invalid` and `The filter 'placement-group-name' is invalid`. In that message the fixed text is `The filter '` ... `' is invalid` and the only variable part is the filter name copied from the request — the emulator's wording is right for an unknown filter, but these filters are not unknown, so the refusal itself is the defect.
Why: a client cannot ask EC2 which instances consumed a capacity reservation, or which instances landed in a placement group/partition — the two canonical server-side queries for those features. Every such read fails hard instead of returning a set, so consumption and placement cannot be verified at all.
Fix: the DescribeInstances handler's filter validation / filter-name table (the allow-list that decides `invalid`), plus the matching predicate for these three names over instance state.
Traces: ec2.capres-targeted-reservation-consumed-by-a-guest-that-really-boots, ec2.placement-partition-number-above-the-group-count-rejected, ec2.placement-spread-group-rejects-more-instances-than-one-zone-can-place.

=== Cause 2 of 5: a CloudFormation-created AWS::EC2::Instance reports a name, not an instance id ===
AWS: recorded behavior only (the EC2 Smithy model states the shape, not CloudFormation's id assignment). For AWS::EC2::Instance, CloudFormation's physical resource id — what DescribeStackResources reports and what `Ref` resolves to — IS the EC2 instance id, of the form `i-` followed by 17 hex characters, and feeding it straight into DescribeInstances(InstanceIds=[...]) succeeds.
Emulator: the stack reports a NAME-shaped value instead — `<stack-name>-<LogicalId>-<suffix>` — and DescribeInstances then refuses it with HTTP 400 / `InvalidInstanceID.Malformed`, message `Invalid id: "<value>"`. The replay shows values such as `sim2real-ec2mr-byvpc-e2e-...-InstanceA1-...`, `sim2real-ec2mr-dvpc-...-Instance-...` and `sim2real-ec2mr-pub-...-PublicInstance-...`; the stack name, logical id and suffix are per-run values, the shape (`CFN name` rather than `i-<17 hex>`) is the requirement. The refusal itself is correctly coded and worded for a malformed id — the defect is that the id handed out is not an instance id.
Why: nothing a stack builds can be joined back to EC2. Every read, tag, terminate or wait keyed on the stack's instance id fails at the first call, so a deploy phase can never complete.
Fix: the CloudFormation resource provider for AWS::EC2::Instance — the physical-resource-id it records on create (and therefore what `Ref` and DescribeStackResources return). Related but distinct from the open issue 20261025T150742Z-cloudformation-ec2-rds-aws-ec2-subnet-s-physical.md, which covers AWS::EC2::Subnet, not AWS::EC2::Instance.
Traces: ec2.ec2mr-byvpc-e2e-multi-vpc-grouping-lifecycle, ec2.ec2mr-dvpc-e2e-without-default-vpc-lifecycle, ec2.ec2mr-pub-e2e-public-subnets-lifecycle.

=== Cause 3 of 5: stack Outputs carry no OutputValue, so an EC2 estate cannot be read back ===
AWS: recorded behavior only. DescribeStacks on a stack that reached CREATE_COMPLETE returns, for every declared Output, an entry carrying both `OutputKey` and `OutputValue` (plus optional Description/ExportName). OutputValue is the resolved value and is always present once the stack is complete.
Emulator: the Output entries omit `OutputValue` entirely, so a client indexing it raises `KeyError: 'OutputValue'` (the replay reports it as `[phase 1/4 deploy] KeyError: 'OutputValue'`).
Why: an EC2 estate declared by a template cannot be discovered — the VPC, subnet and instance ids the stack publishes are unreachable, so every downstream EC2 call is impossible even though the stack claims success.
Fix: the CloudFormation DescribeStacks output serializer and the output-resolution step that should populate it. Open issues 20261029T175114Z-cloudformation-stack-outputs-carry-no-outputvalue-an-asg.md and 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md describe the same symptom for other estates; neither carries these checks' replay keys.
Traces: ec2.e2e-ec2mr-xrg-cross-region-connectivity-lifecycle, ec2.ec2mr-inv-e2e-all-regions-instance-inventory-lifecycle.

=== Cause 4 of 5: AttachVerifiedAccessTrustProvider's not-found message is emulator-authored prose ===
AWS: Smithy: AttachVerifiedAccessTrustProvider takes required VerifiedAccessInstanceId and VerifiedAccessTrustProviderId. A trust-provider id that does not exist is refused with EC2's resource-not-found message template for that resource kind: the fixed text `The verified-access-trust-provider ID '` ... `' does not exist`, with the id taken verbatim from the request.
Emulator: answers a different message for the same refusal — `VerifiedAccessTrustProvider <id> does not exist` (emulator prose: no `The ... ID '...' does not exist` template, no quoting of the id, resource kind spelled in PascalCase instead of the hyphenated EC2 resource token).
Why: clients and operators that match on EC2's documented message text (log scrapers, error-classification layers, retry rules keyed on message shape) do not recognise the refusal.
Fix: the AttachVerifiedAccessTrustProvider handler's not-found refusal, or the shared EC2 not-found message template it should be using for the `verified-access-trust-provider` resource kind.
Traces: ec2.op-contract-attachVerifiedAccessTrustProvider.

=== Cause 5 of 5: ModifyCapacityReservation on a cancelled reservation answers the wrong error code ===
AWS: recorded behavior only for the code (the EC2 Smithy model declares no modelled errors on ModifyCapacityReservation; EC2 carries refusals as ec2-query errors). A ModifyCapacityReservation against a reservation that is no longer active (already cancelled) is refused with error code `InvalidStateTransition` — the state-transition class, not the generic wrong-state class.
Emulator: refuses the same call with error code `IncorrectState`.
Why: a client branching on the error code cannot tell a capacity reservation that was cancelled from the unrelated resources that genuinely answer IncorrectState; retry and reconciliation logic keyed on InvalidStateTransition never fires.
Fix: the ModifyCapacityReservation handler's state check and the error code it raises.
Traces: ec2.op-contract-modify-capacity-reservation-state-conflict.

## Evidence
Cause 1 — expected: HTTP 200 from DescribeInstances with Filters=[{Name:'capacity-reservation-id'|'placement-group-name',...}]. actual: HTTP 400 InvalidParameterValue, "The filter 'capacity-reservation-id' is invalid" / "The filter 'placement-group-name' is invalid".

Cause 2 — expected: the stack's physical id for AWS::EC2::Instance is an instance id (`i-` + 17 hex) that DescribeInstances accepts. actual: a CFN name-shaped value, refused with InvalidInstanceID.Malformed, 'Invalid id: "sim2real-ec2mr-byvpc-e2e-fc2055e8c020-InstanceA1-65B840C36ED9"' (and the dvpc/pub equivalents).

Cause 3 — expected: every DescribeStacks Output entry of a CREATE_COMPLETE stack carries OutputValue. actual: the key is absent; the replay aborts in the deploy phase with KeyError: 'OutputValue'.

Cause 4 — expected message: "The verified-access-trust-provider ID 'vatp-<id>' does not exist" (fixed text; the id is the request's). actual message: "VerifiedAccessTrustProvider vatp-<id> does not exist".

Cause 5 — expected error code: InvalidStateTransition. actual error code: IncorrectState.

## Steps to Reproduce
1. Re-replay the behavioral suite and select the check ids in trace_ids.
2. Cause 1: create a capacity reservation and a placement group, launch into them, then call DescribeInstances with Filters=[{'Name':'capacity-reservation-id','Values':[<cr-id>]}] and again with Filters=[{'Name':'placement-group-name','Values':[<group>]}]; observe InvalidParameterValue instead of a 200 result set.
3. Cause 2: deploy a template containing an AWS::EC2::Instance, read its physical resource id (DescribeStackResources or a Ref output), and pass it to DescribeInstances(InstanceIds=[...]); observe InvalidInstanceID.Malformed because the id is a CFN name.
4. Cause 3: deploy any stack declaring Outputs, wait for CREATE_COMPLETE, call DescribeStacks and read Stacks[0].Outputs[0]['OutputValue']; observe the key is missing.
5. Cause 4: call AttachVerifiedAccessTrustProvider with a live VerifiedAccessInstanceId and a nonexistent VerifiedAccessTrustProviderId; compare the error message text.
6. Cause 5: create a capacity reservation, cancel it, then call ModifyCapacityReservation on it; compare the error code.
