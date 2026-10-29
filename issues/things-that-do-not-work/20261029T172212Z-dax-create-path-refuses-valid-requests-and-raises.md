Issue-Source: trace-sweep-divergence
Issue-Trace: dax-create-cluster-describe-events-validation
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dax-create-cluster-describe-events-validation
Issue-Shape-Id: com.amazonaws.dax#AmazonDAXV3
Issue-Key: dax|CreateSubnetGroup|2xx->4xx|status
Issue-Key: dax|CreateCluster|4xx->4xx|$.__type
Issue-Key: dax|CreateCluster|4xx->4xx|$.message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DAX create path refuses valid requests and raises the wrong fault

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubnetGroup

## Description
Two causes in the DAX control-plane create path. Both are about which fault the create operations raise: one refuses a request AWS accepts, the other answers a client-side validation error where AWS answers a resource-not-found fault. A fixer must close BOTH before submitting.

Cause 1 of 2: CreateSubnetGroup refuses a subnet group whose SubnetIds AWS accepts (fixture dax-create-cluster-describe-events-validation, step setup#5).
AWS: Smithy: CreateSubnetGroup input SubnetGroupName (String, @required) and SubnetIds (SubnetIdentifierList, @required); success status 200 with output member SubnetGroup. AWS creates the group for the subnet ids the request names, when those subnets exist in the caller's account/VPC, and answers 200 with the SubnetGroup (name lower-cased, VpcId and each Subnet's SubnetAvailabilityZone filled in). The modeled rejections for a bad identifier are InvalidSubnet / SubnetNotAllowedFault; the recording shows a plain 200 for this request, so no rejection is in play here.
Emulator: answers HTTP 400 instead of 200. The handler resolves every element of SubnetIds against a SYNTHETIC region network: services/service-com-amazonaws-dax-amazondaxv3/src/state.rs region_subnets() derives one subnet per AZ suffix with subnet_id = "subnet-" + the first SUBNET_ID_HEX_LEN hex digits of SHA256(availability_zone), and resolve_subnet() accepts only an identifier equal to one of those digests. Any other identifier is collected as unresolved and the create is refused with InvalidParameterValueException carrying state.rs invalid_subnets(): fixed text "Some input subnets in :[" + the unresolved identifiers joined by ", " + "] are invalid." Because the derived ids are digests no client can know or discover through any DAX operation, every CreateSubnetGroup that names client-chosen subnet ids is refused.
Why: a client cannot create a subnet group at all, so everything downstream of it (CreateCluster with SubnetGroupName, DescribeSubnetGroups, the cluster lifecycle) is unreachable; the 400 surfaces as an InvalidParameterValueException naming the caller's own perfectly well-formed subnet ids.
Fix: CreateSubnetGroup's handler/validation in services/service-com-amazonaws-dax-amazondaxv3/src/ops/create_subnet_group.rs together with the subnet resolution it calls in src/state.rs (resolve_subnet / region_subnets): a well-formed subnet identifier must resolve rather than be matched against an undiscoverable derived digest set, so the create stores the group and answers 200.
Traces: dax-create-cluster-describe-events-validation#setup#5.

Cause 2 of 2: CreateCluster naming a parameter group that does not exist answers a ClusterName validation error instead of ParameterGroupNotFoundFault (fixture dax-create-cluster-describe-events-validation, steps task#7 and task#8).
AWS: Smithy: CreateCluster input member ParameterGroupName (String) -- "The parameter group to be associated with the DAX cluster"; modeled error ParameterGroupNotFoundFault -- "The specified parameter group does not exist." The recorded response for a CreateCluster naming an unknown parameter group is HTTP 400 with __type "ParameterGroupNotFoundFault" and message "ParameterGroup not found: <ParameterGroupName>" -- fixed text "ParameterGroup not found: " followed by the ParameterGroupName taken from the request (the recording's own value is only one instance of that variable part). The recording also pins PRECEDENCE: the parameter group is resolved before the ClusterName format rule is applied, so a request that would fail both reports the parameter-group fault.
Emulator: status 400 matches, body does not. It answers __type "InvalidParameterValueException" with message "ClusterName must contain from 1 to 20 alphanumeric characters or hyphens". src/ops/create_cluster.rs calls state::cluster_name_violation() as its FIRST check and returns InvalidParameterValueException, and it never resolves ParameterGroupName at all: input.parameter_group_name is copied straight into the stored ClusterRecord, so no path in CreateCluster can ever raise ParameterGroupNotFoundFault. Separately, the message template the fault would use is wrong even once the lookup exists: state.rs parameter_group_not_found_message() returns the fixed string "Parameter Group not found." and deliberately echoes no name, where AWS words it "ParameterGroup not found: <name>" with the requested name interpolated.
Why: a client probing for a missing parameter group gets InvalidParameterValueException instead of ParameterGroupNotFoundFault, so error-code-based handling (retry/create-the-group-then-retry) never fires, and the message blames an unrelated input member. A cluster is also accepted with a dangling ParameterGroupName when the name does pass the format rule.
Fix: CreateCluster's handler in services/service-com-amazonaws-dax-amazondaxv3/src/ops/create_cluster.rs -- resolve ParameterGroupName against the store (including the reserved default group) and raise ParameterGroupNotFoundFault before the ClusterName format check -- plus the shared fault message in src/state.rs (parameter_group_not_found_message / parameter_group_not_found), which must interpolate the requested name.
Traces: dax-create-cluster-describe-events-validation#task#7, dax-create-cluster-describe-events-validation#task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1 (6 fixtures, 42 steps, 24 passed, 18 mismatched, score 57.1%).

Cause 1 -- fixture dax-create-cluster-describe-events-validation, step setup#5, operation CreateSubnetGroup, verdict mismatch:
  expected status 200, actual status 400 (detail: "status: expected 200, got 400").
  Cluster key service=dax|CreateSubnetGroup|status=200->400|keys=status; affected: dax-create-cluster-describe-events-validation#setup#5.
  Emulator's refusal comes from src/state.rs invalid_subnets(): InvalidParameterValueException, "Some input subnets in :[<unresolved ids>] are invalid."

Cause 2 -- fixture dax-create-cluster-describe-events-validation, step task#7, operation CreateCluster, verdict mismatch (status 400 expected and returned):
  $.__type: expected 'ParameterGroupNotFoundFault', got 'InvalidParameterValueException'
  $.message: expected 'ParameterGroup not found: <ParameterGroupName>' (recorded with the probe's own group name), got 'ClusterName must contain from 1 to 20 alphanumeric characters or hyphens'
  Cluster key service=dax|CreateCluster|status=400->400|keys=$.__type,$.message; affected: dax-create-cluster-describe-events-validation#task#7, dax-create-cluster-describe-events-validation#task#8.
  Code: src/ops/create_cluster.rs lines 237-244 (cluster_name_violation first) and line 389 (parameter_group_name stored with no lookup); src/state.rs lines 931-952 (cluster_name_violation) and 966-968 / 1115-1119 (ParameterGroupNotFoundFault message with no name interpolated).

## Steps to Reproduce
1. Boot the emulator from the mainline checkout and replay fixture dax-create-cluster-describe-events-validation from trace set simparity-wire-aws-v1.
2. Step setup#5 (CreateSubnetGroup): send CreateSubnetGroup with a SubnetGroupName and a list of well-formed VPC subnet ids. Expect 200 with the SubnetGroup; the emulator answers 400 InvalidParameterValueException "Some input subnets in :[...] are invalid."
3. Steps task#7/task#8 (CreateCluster): send CreateCluster with ParameterGroupName set to a parameter group that does not exist in the store. Expect 400 with __type ParameterGroupNotFoundFault and message "ParameterGroup not found: <that ParameterGroupName>"; the emulator answers 400 with __type InvalidParameterValueException and message "ClusterName must contain from 1 to 20 alphanumeric characters or hyphens". The parameter-group resolution must precede the ClusterName format check, as the recording shows.
