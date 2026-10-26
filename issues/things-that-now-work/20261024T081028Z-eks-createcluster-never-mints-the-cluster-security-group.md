Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__eks.ekscc-cluster-security-group-minted-in-subnets-vpc-and-released-on-delete ec2__eks.ekscc-create-cluster-with-nonexistent-subnet-rejected
Issue-Key: ec2__eks.ekscc-cluster-security-group-minted-in-subnets-vpc-and-released-on-delete|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: ec2__eks.ekscc-create-cluster-with-nonexistent-subnet-rejected|run#10|AssertionError|assert (code, message) == want, (
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# EKS CreateCluster never mints the cluster security group in EC2, and its wrapped EC2 subnet-not-found message omits the request-id/proxy tail

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Two net-new root causes in the EKS emulator, both on the EKS-to-EC2 edge.

Cause 1 of 2: CreateCluster does not mint (and DeleteCluster therefore cannot release) the cluster security group in EC2.
AWS: recorded behavior only (the eks Smithy model describes the envelope, not the cross-service effect). Creating a cluster mints an owned cluster security group in the VPC the given subnets belong to: while the cluster is still CREATING, EC2 holds exactly ONE security group tagged aws:eks:cluster-name=<cluster>, named eks-cluster-sg-<cluster>-*, inside that VPC and tagged as owned by the cluster. After DeleteCluster that group is gone from EC2 as well.
Emulator: the group never appears in EC2, so the check's poll makes no progress and the harness kills it at its 60 s budget with "Check timed out after 60.0s" (no exception frame - the signature of a poll waiting on a state that never arrives).
Why: the cluster security group is the handle every node group, add-on and cluster-to-node rule attaches to. Against the emulator it cannot be found, so nothing can be authorised onto the cluster's data path; and because it is never created, the delete-side release contract is untestable too. A client polling for the group (the documented pattern, since it appears during CREATING) hangs.
Fix: the CreateCluster handler's cross-service provisioning in services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_cluster.rs (and the matching release in delete_cluster.rs) - mint the group in the EC2 service state, in the subnets' VPC, with the aws:eks:cluster-name and ownership tags and the eks-cluster-sg-<name>- name prefix.
Traces: ec2__eks.ekscc-cluster-security-group-minted-in-subnets-vpc-and-released-on-delete.

Cause 2 of 2: the wrapped EC2 error EKS returns for an unknown subnet is missing its request-id/proxy tail.
AWS: recorded behavior only. EKS resolves each subnet against EC2 and, when one does not exist, refuses the create with InvalidParameterException whose message EMBEDS the underlying EC2 error in AWS's wrapped form: "The subnet ID '<id>' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <id>; Proxy: null)". The harness normalises the request id itself, so the required part is the SHAPE of the tail - the Request ID and Proxy fields are present - not any particular id.
Emulator: returns the right code and the right head, but truncates the tail: "... Error Code: InvalidSubnetID.NotFound)" with no "; Request ID: ...; Proxy: null".
Why: the wrapped tail is how a caller correlates an EKS rejection with the EC2 call that actually failed (and with CloudTrail/support). Tooling that parses the wrapped form to extract the underlying request id finds nothing. The error code is correct, so this is a message-rule divergence only.
Fix: the subnet-resolution error construction in services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/create_cluster.rs - emit the full wrapped-EC2 tail including the Request ID of the inner call and Proxy.
Traces: ec2__eks.ekscc-create-cluster-with-nonexistent-subnet-rejected.

## Evidence
Cause 1 - ec2__eks.ekscc-cluster-security-group-minted-in-subnets-vpc-and-released-on-delete: "Error: Check timed out after 60.0s" (no frame). The check mints its own VPC (estate:none) and polls EC2 for exactly one security group tagged aws:eks:cluster-name=<name>, named eks-cluster-sg-<name>-*, in the subnets' VPC while the cluster is CREATING.
Cause 2 - AssertionError: an unknown subnet must be refused with ('InvalidParameterException', "The subnet ID 'subnet-0fc288e4d89070000' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound; Request ID: <request-id>; Proxy: null)"); got ('InvalidParameterException', "The subnet ID 'subnet-0fc288e4d89070000' does not exist (Service: AmazonEC2; Status Code: 400; Error Code: InvalidSubnetID.NotFound)"). The harness already substitutes <request-id>, so the divergence is the missing Request ID/Proxy fields, not their values.
Cluster names, subnet ids and request ids differ per run and are examples, not requirements.

## Steps to Reproduce
1. git_clone(), then run_behavioral_suite().
2. Cause 1: create a VPC with subnets, call eks.CreateCluster on those subnets, then poll ec2.DescribeSecurityGroups filtered on tag aws:eks:cluster-name=<cluster> -> expect exactly one group named eks-cluster-sg-<cluster>-* in that VPC; observe none, and the poll never completes. Then DeleteCluster and confirm the group is released.
3. Cause 2: call eks.CreateCluster naming a subnet id that does not exist in EC2 -> expect InvalidParameterException whose wrapped message carries "; Request ID: ...; Proxy: null"; observe the tail truncated after the Error Code.
