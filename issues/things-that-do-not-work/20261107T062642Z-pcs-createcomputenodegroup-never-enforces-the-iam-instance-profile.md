Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pcs.compute-node-group-and-queue-lifecycle
Issue-Shape-Id: com.amazonaws.pcs#AWSParallelComputingService
Issue-Key: pcs.compute-node-group-and-queue-lifecycle|run#17|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code} ({message})"
Issue-Commit: 3bb19599b572b08e2f28b94b32c7f468b00aeacc

# pcs CreateComputeNodeGroup never enforces the IAM instance-profile role-naming rule: a role that is neither AWSPCS-prefixed nor on the /aws-pcs/ path is accepted and the node group is created

> _Note: this issue was identified against the code base at commit `3bb19599b572b08e2f28b94b32c7f468b00aeacc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputeNodeGroup

## Description
One net-new root cause in the PCS emulator (services/service-com-amazonaws-pcs-awsparallelcomputingservice), found by replaying the behavioral suite.

Model consulted: services/service-com-amazonaws-pcs-awsparallelcomputingservice/smithy-model/com-amazonaws-pcs-awsparallelcomputingservice.json (sdkId PCS, protocol awsJson1_0).

=== Cause 1 of 1: the iamInstanceProfileArn role-naming convention is never validated ===
Failing check: pcs.compute-node-group-and-queue-lifecycle

AWS: Smithy: CreateComputeNodeGroupRequest.iamInstanceProfileArn -> InstanceProfileArn, required, @pattern ^arn:aws([a-zA-Z-]{0,10})?:iam::[0-9]{12}:instance-profile/([!-~]{1,510}/)?([\w+=,.@-]{1,128})$; its documentation states the rule the service enforces: "The role contained in your instance profile must have the pcs:RegisterComputeNodeGroupInstance permission and the role name must start with AWSPCS or must have the path /aws-pcs/." CreateComputeNodeGroup models ValidationException (errorKind client, httpError 400, required members message and reason, optional fieldList). A request whose instance profile carries a role that satisfies neither half of that naming/path convention is refused synchronously by PCS's own request validation — no node group is created, and nothing is left behind under the requested name. The refusal message is FIXED text that echoes no caller-supplied value (it deliberately does not name the ARN): "AWS PCS can't access a role associated with the instance profile because the role ARN is invalid. For more information, see CreateComputeNodeGroup or UpdateComputeNodeGroup in the API reference." Every word of that sentence is fixed; there are no request- or state-derived parts. The requirement is therefore: HTTP 400, error code ValidationException, that exact message, and no created resource.

Emulator: the call SUCCEEDS. Replaying the check, CreateComputeNodeGroup with an instance profile whose role name is neither AWSPCS-prefixed nor on the /aws-pcs/ path returned a 200 with a computeNodeGroup in CREATING, so the check recorded no error at all ("expected ValidationException, got None (None)"). Reading the handler confirms it: services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_compute_node_group.rs validates computeNodeGroupName, the stored list sizes, scalingConfiguration min<=max, the tag ceiling, cluster existence/ACTIVE status, duplicate group name and the per-cluster group quota, and then stores input.iam_instance_profile_arn verbatim onto the record — the ARN is never inspected for the role-naming convention on any path.

Why: a client gets the opposite verdict on an input AWS always rejects. Provisioning code (and any IaC layer) that relies on PCS rejecting a mis-named compute-node role sees a node group created and driven to ACTIVE, so the misconfiguration is never surfaced where AWS surfaces it, and a test asserting the 400/ValidationException contract passes on AWS and fails on the emulator — or worse, passes against the emulator and ships a role PCS will refuse. The effect-absence half of the contract is broken too: ListComputeNodeGroups reports a group under a name AWS leaves unused.

Fix: the CreateComputeNodeGroup handler's validation (services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_compute_node_group.rs), with the role-name/path rule expressed once beside the other PCS validators in the crate's state module (next to resource_name_violation / field_validation) so the create path and any later UpdateComputeNodeGroup path cannot disagree about it; the refusal must be the modeled ValidationException (400) carrying the service's fixed sentence above and the modeled reason member. Note: ops/update_compute_node_group.rs is still a generated scaffold returning not_implemented("UpdateComputeNodeGroup"), so the same rule needs applying there when that stub is implemented — that stub itself is not part of this issue.

Related but distinct open issues (neither covers this rule): 20261023T123102Z-pcs-fidelity-bundle-5-divergences-not-found-message.md Cause 3 is about the ORDER in which CreateComputeNodeGroup resolves the cluster vs. validating that the referenced instance profile EXISTS; 20261106T101923Z-pcs-createcluster-refuses-a-valid-request-on-an.md is about CreateCluster. This cause is the role NAMING/PATH convention, which the emulator does not implement at all.

Traces: pcs.compute-node-group-and-queue-lifecycle.

## Evidence
Behavioral suite: pcs 6/13 passed (46%), 7 failed.

[mismatch] pcs.compute-node-group-and-queue-lifecycle:
  File "check_pcs_compute_node_group_and_queue_lifecycle.py", line 70, in run
    assert code == "ValidationException", f"expected ValidationException, got {code} ({message})"
  AssertionError: expected ValidationException, got None (None)

Expected: HTTP 400, Error.Code = ValidationException, Error.Message = "AWS PCS can't access a role associated with the instance profile because the role ARN is invalid. For more information, see CreateComputeNodeGroup or UpdateComputeNodeGroup in the API reference." (fixed text, no variable parts), and no compute node group under the requested name (the check's follow-up assertion `all(g["name"] != cng_name_bad for g in list_compute_node_groups(...))`, which was never reached).
Actual: HTTP 200 with a computeNodeGroup record (status CREATING) — no error raised at all, so the check's `code`/`message` stayed None.

Code in the checkout: services/service-com-amazonaws-pcs-awsparallelcomputingservice/src/ops/create_compute_node_group.rs — validation runs over computeNodeGroupName (line ~233), stored list sizes (~259), scalingConfiguration (~272), tag count (~290), cluster resolution/status (~305, ~357), duplicate name (~364) and the group quota (~384); input.iam_instance_profile_arn is only ever stored onto ComputeNodeGroupRecord (line ~421). No role-name/path check exists anywhere on the path.

## Steps to Reproduce
Boot the emulator and point a botocore/AWS SDK PCS client at it (awsJson1_0, sigv4).

1. Create a VPC, a subnet and that VPC's default security group in EC2, and an EC2 launch template whose LaunchTemplateData carries SecurityGroupIds.
2. pcs.create_cluster(clusterName=<name>, scheduler={'type':'SLURM','version':'25.05'}, size='SMALL', networking={'subnetIds':[<subnet>],'securityGroupIds':[<sg>]}) and poll get_cluster until status == ACTIVE.
3. In IAM, create a role whose name does NOT start with AWSPCS and is NOT on the path /aws-pcs/ (e.g. 'myteam-badrole-<suffix>'), attach arn:aws:iam::aws:policy/AWSPCSComputeNodePolicy, create an instance profile of the same name and add the role to it.
4. pcs.create_compute_node_group(clusterIdentifier=<cluster id>, computeNodeGroupName=<name>, subnetIds=[<subnet>], customLaunchTemplate={'id':<lt id>,'version':'1'}, iamInstanceProfileArn=<that profile's ARN>, scalingConfiguration={'minInstanceCount':0,'maxInstanceCount':0}, instanceConfigs=[{'instanceType':'c5.large'}]).
   Expect: ClientError with HTTP 400, Code ValidationException and Message "AWS PCS can't access a role associated with the instance profile because the role ARN is invalid. For more information, see CreateComputeNodeGroup or UpdateComputeNodeGroup in the API reference."
   Observe: HTTP 200 and a computeNodeGroup in CREATING.
5. pcs.list_compute_node_groups(clusterIdentifier=<cluster id>) — expect no group under that name; observe the group created in step 4.
6. Positive control (must keep working): repeat step 3-4 with a role/profile named 'AWSPCS-<suffix>' (or on path /aws-pcs/) and confirm the node group is created and reaches ACTIVE.

Full replay: run the behavioral suite and re-run pcs.compute-node-group-and-queue-lifecycle.
