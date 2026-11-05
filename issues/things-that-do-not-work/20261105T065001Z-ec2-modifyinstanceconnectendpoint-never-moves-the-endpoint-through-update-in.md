Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-modify-instance-connect-endpoint
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-modify-instance-connect-endpoint|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: 57cca9dfb40cff43666727c7926823b091294572

# ModifyInstanceConnectEndpoint never moves the endpoint through update-in-progress to update-complete, so a successful modify is unobservable and clients wait forever

> _Note: this issue was identified against the code base at commit `57cca9dfb40cff43666727c7926823b091294572`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyInstanceConnectEndpoint

## Description
Cause 1 of 1: a successful ModifyInstanceConnectEndpoint leaves the EC2 Instance Connect Endpoint's reported State frozen at its create-time value, so the documented update state machine never runs. Failing check id: ec2.op-contract-modify-instance-connect-endpoint.

AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models Ec2InstanceConnectEndpointState as an enum whose values are exactly create-in-progress, create-complete, create-failed, delete-in-progress, delete-complete, delete-failed, update-in-progress, update-complete, update-failed, and Ec2InstanceConnectEndpoint$State is documented as "The current state of the EC2 Instance Connect Endpoint". The three update-* members exist for this operation: ModifyInstanceConnectEndpoint ("Modifies the specified EC2 Instance Connect Endpoint", output Return=true on success) applies the change asynchronously, so after an accepted modify DescribeInstanceConnectEndpoints reports the endpoint as update-in-progress and then settles it at update-complete (or update-failed when the change cannot be applied), with the changed members — here SecurityGroupIds, the "new set of groups … replaces the current set" — readable once the endpoint has settled. ModifyInstanceConnectEndpoint models no errors, so a request the service accepts is reported only through that state transition. The recorded check is written to exactly that contract: it calls ModifyInstanceConnectEndpoint with SecurityGroupIds=[sg2] and then polls DescribeInstanceConnectEndpoints until State is in (update-complete, update-failed), asserting update-complete and SecurityGroupIds==[sg2].

Emulator: ModifyInstanceConnectEndpoint is implemented and returns Return=true (200), and it does mutate the stored view in place — ip_address_type, preserve_client_ip and security_group_ids — but it writes NO update milestone: the comment at services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_instance_connect_endpoint.rs (handler body, the write block that sets stored.view.*) states that "the creation's clock-owed move keeps the instant it was given, so a modify neither restarts nor completes it", and the record's State is derived purely from the create/delete milestones the create path set (create_complete_at = now + TRANSITION_DELAY, delete_complete_at = None, in create_instance_connect_endpoint.rs). The endpoint therefore keeps reporting create-complete after the modify and never reports update-in-progress or update-complete; no code path in the crate can ever produce either of the two update states the model declares. The replayed check consequently never leaves its poll loop and the suite records it as a mismatch with the detail "Check timed out after 60.0s" — the emulator answered every request, it just never advanced the state the response contract requires.

Why: a client that follows the documented lifecycle after modifying an EC2 Instance Connect Endpoint — waiting for the endpoint to leave update-in-progress before it trusts the new security groups, which is what CloudFormation-style reconcilers, CDK custom resources and hand-written waiters all do — blocks forever against this emulator, because the terminal state it waits for is unreachable. A client that instead polls for "not update-in-progress" sees create-complete immediately and so cannot distinguish an applied modify from one still in flight, and a failed modify can never be reported as update-failed at all. The only observable signal of the change is the mutated member set, which is indistinguishable from the pre-modify endpoint for a caller that re-sends the same groups.

Fix: the operation's state, in the EC2 Instance Connect Endpoint record and in the ModifyInstanceConnectEndpoint handler that writes it. The record needs an update milestone alongside its create/delete milestones (the same clock-owed TRANSITION_DELAY pattern the create path already uses for create-in-progress -> create-complete), the handler must set it on every accepted modify before it returns Return=true, and the record's state derivation must report update-in-progress until that instant and update-complete after it — while leaving the create and delete milestones, and the delete path's precedence over them, untouched. DescribeInstanceConnectEndpoints reads the derived state, so it needs no change of its own.

Traces: ec2.op-contract-modify-instance-connect-endpoint (suite verdict: mismatch; detail "Check timed out after 60.0s"; the check creates a VPC, a subnet and two security groups, creates an endpoint with sg1, waits for create-complete, calls ModifyInstanceConnectEndpoint with SecurityGroupIds=[sg2], and then polls for State in (update-complete, update-failed)). Handler: services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_instance_connect_endpoint.rs; create-time milestone it inherits: services/service-com-amazonaws-ec2-amazonec2/src/ops/create_instance_connect_endpoint.rs.

## Evidence
Expected (real AWS / Smithy model): after ModifyInstanceConnectEndpoint(InstanceConnectEndpointId=<eice-id>, SecurityGroupIds=[<sg-id>]) returns Return=true, DescribeInstanceConnectEndpoints(InstanceConnectEndpointIds=[<eice-id>]) reports Ec2InstanceConnectEndpoint.State = "update-in-progress" and then settles at "update-complete" (or "update-failed"), with SecurityGroupIds == [<sg-id>]. Smithy: Ec2InstanceConnectEndpointState enum values include update-in-progress | update-complete | update-failed; ModifyInstanceConnectEndpoint declares no errors and returns Return (Boolean).

Actual (emulator): ModifyInstanceConnectEndpoint answers 200 with Return=true and updates the stored SecurityGroupIds, but DescribeInstanceConnectEndpoints keeps reporting State = "create-complete" indefinitely. The states update-in-progress, update-complete and update-failed are never produced by any path in the crate: the record carries only create_complete_at and delete_complete_at, and modify_instance_connect_endpoint.rs deliberately leaves both alone ("a modify neither restarts nor completes it"). The replayed behavioral check polls for State in (update-complete, update-failed) and is killed by the harness: "Check timed out after 60.0s".

## Steps to Reproduce
1. CreateVpc with a CIDR block, then CreateSubnet in it.
2. CreateSecurityGroup twice in that VPC -> sg1, sg2.
3. CreateInstanceConnectEndpoint(SubnetId=<subnet>, SecurityGroupIds=[sg1]); poll DescribeInstanceConnectEndpoints until State == "create-complete" (this succeeds).
4. ModifyInstanceConnectEndpoint(InstanceConnectEndpointId=<eice-id>, SecurityGroupIds=[sg2]) -> 200, Return=true.
5. Poll DescribeInstanceConnectEndpoints(InstanceConnectEndpointIds=[<eice-id>]) for State in ("update-complete", "update-failed").
   Observed: State stays "create-complete" on every poll, so the loop never terminates; the behavioral check ec2.op-contract-modify-instance-connect-endpoint is cut off with "Check timed out after 60.0s".
   Expected: State is "update-in-progress" immediately after step 4 and "update-complete" shortly after, with SecurityGroupIds == [sg2].
