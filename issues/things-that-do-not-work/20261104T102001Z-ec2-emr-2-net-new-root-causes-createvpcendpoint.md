Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode
Issue-Key: ec2__emr|CreateVpcEndpoint|NotImplemented
Issue-Key: ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: a57dea262f41459244daf18c6d5b4cdf00e6a648

# ec2__emr: 2 net-new root causes — CreateVpcEndpoint refuses a gateway endpoint that names RouteTableIds with 501 NotImplemented, and a cluster's Spark steps never reach a terminal state

> _Note: this issue was identified against the code base at commit `a57dea262f41459244daf18c6d5b4cdf00e6a648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint, AddJobFlowSteps

## Description
Two distinct root causes clustered from this run's ec2__emr failures; each is numbered and carries its own failing check ids.

=== Cause 1 of 2: EC2 CreateVpcEndpoint refuses any request that names RouteTableIds with 501 NotImplemented, so the EMR fleet-propagation checks die in VPC setup ===
AWS: Smithy (com.amazonaws.ec2#AmazonEC2): CreateVpcEndpoint input carries RouteTableIds (VpcEndpointRouteTableIdList, xmlName RouteTableId) next to VpcId and ServiceName; it is a normal member of a Gateway-endpoint request. AWS answers HTTP 200 with CreateVpcEndpointResult.VpcEndpoint whose RouteTableIds echo the associated route tables, and installs the prefix-list route in each named table. The operation models no error for this input; a refusal would be a 400 with a Client.* code, never 501 NotImplemented.
Emulator: HTTP 501, error code 'NotImplemented', message 'operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator' — entirely emulator-authored fixed text that AWS never emits for this operation.
Why: the instance-fleet propagation scenarios (which EC2 instance a CORE fleet launches, its subnet, security group and EMR-managed tags; the memory derivation of its executors and driver) abort before the first EMR call, and a client doing the same AWS-accepted wiring gets a non-retryable 501.
Fix: the ec2 crate's CreateVpcEndpoint handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint.rs) — associate the named route tables and serialize RouteTableIds on the returned VpcEndpoint instead of refusing.
Note: open issues 20261104T075553Z-ec2-emr-iam-createvpcendpoint-refuses-a-gateway-endpoint.md, 20261104T080454Z-emr-events-sqs-ec2-createvpcendpoint-refuses-any-request-carrying-routetableids-with.md and 20261104T082656Z-emr-glue-ec2-createvpcendpoint-refuses-any-request-carrying.md describe this refusal under other service headings; none carries the ec2__emr check keys below, so those checks must be re-replayed to confirm the fix.
Traces: ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags, ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver.

=== Cause 2 of 2: steps submitted to a WAITING instance-fleet cluster never reach a terminal state, so the deploy-mode scenario never settles ===
AWS: recorded behavior only. Three spark-submit steps added with AddJobFlowSteps to a WAITING cluster are each picked up and reach a terminal state within the scenario's deadline — ListSteps reports COMPLETED or FAILED per step (the oversized cluster-mode driver FAILED, the right-sized cluster-mode and the client-mode submits COMPLETED).
Emulator: the check's poll for 'all three steps reach a terminal state' never satisfies and the check ends with 'Check timed out after 60.0s'; no step is ever observed leaving its non-terminal state. Hypothesis: steps are recorded but never executed, so no step reaches a terminal state (the same family as the open issue 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md, which does not carry this check key).
Why: a client that submits steps and waits on their terminal state waits forever, and the AWS distinction this scenario rests on — an oversized driver fails only in cluster deploy mode, not in client mode — is unobservable.
Fix: the emr crate's step-execution lifecycle (the AddJobFlowSteps / ListSteps state machine and whatever drives step state forward).
Traces: ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode.

## Evidence
Behavioral suite replay against the emulator built from this checkout (ec2__emr 0/4 passed).

Cause 1 — checks ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags and ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver [error]:
  expected: CreateVpcEndpoint(VpcId, VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[<rtb>]) -> HTTP 200 with VpcEndpoint.RouteTableIds=[<rtb>].
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateVpcEndpoint operation: operation CreateVpcEndpoint with route table associations is scaffolded but not implemented in this emulator.
  Fixed divergence: status 501 and code 'NotImplemented' where AWS answers 200; the message is wholly emulator-authored.

Cause 2 — check ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode [mismatch]:
  expected: after the cluster reaches WAITING, the three added steps reach {'oversized-cluster-mode': 'FAILED', 'right-sized-cluster-mode': 'COMPLETED', 'oversized-client-mode': 'COMPLETED'}.
  actual:   Error: Check timed out after 60.0s — no step ever reached a terminal state.

## Steps to Reproduce
1. git clone the repo and let the behavioral suite build and boot the emulator from this checkout.
2. Replay checks ec2__emr.core-fleet-launches-requested-type-with-emr-role-tags, ec2__emr.core-fleet-memory-derivation-strands-executors-and-driver and ec2__emr.oversized-driver-spark-step-fails-only-in-cluster-deploy-mode.
3. Cause 1: CreateVpc, CreateSubnet, CreateRouteTable, then CreateVpcEndpoint(VpcId=<vpc>, VpcEndpointType='Gateway', ServiceName='com.amazonaws.<region>.s3', RouteTableIds=[<rtb>]). Expect 200 with VpcEndpoint.RouteTableIds=[<rtb>]; observe 501 NotImplemented.
4. Cause 2: RunJobFlow an instance-fleet cluster (1 MASTER + 1 CORE m5.2xlarge, KeepJobFlowAliveWhenNoSteps=true), wait for WAITING, AddJobFlowSteps with three command-runner spark-submit steps, then poll ListSteps for terminal states; observe that none ever becomes COMPLETED/FAILED.
