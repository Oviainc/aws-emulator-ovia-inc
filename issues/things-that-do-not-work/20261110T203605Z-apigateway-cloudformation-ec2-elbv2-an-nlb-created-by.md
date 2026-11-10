Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back
Issue-Key: apigateway__cloudformation__ec2__elbv2|DescribeLoadBalancers|LoadBalancerNotFound
Issue-Commit: 95197e97fcff53dcc824c7c5147d63782e206b9f

# apigateway__cloudformation__ec2__elbv2: an NLB created by a CREATE_COMPLETE stack is unknown to ELBv2 — DescribeLoadBalancers on the stack's Ref answers LoadBalancerNotFound

> _Note: this issue was identified against the code base at commit `95197e97fcff53dcc824c7c5147d63782e206b9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLoadBalancers

## Description
One net-new root cause (Cause 1 of 1) under the apigateway__cloudformation__ec2__elbv2 heading. Both failing checks in this heading break at the same call, and neither is marked ALREADY FILED or OWNED. Related open filing that does NOT cover these replays: 20261109T132410Z-acm-cloudformation-ec2-elbv2-a-load-balancer-created.md describes the same family (DescribeListeners/LoadBalancerNotFound) but is filed under a different service heading and carries other check ids, so these two failures survive unowned.

Cause 1 of 1: an AWS::ElasticLoadBalancingV2::LoadBalancer declared in a stack that reaches CREATE_COMPLETE is not registered in ELBv2's own state, so the ARN CloudFormation publishes as that resource's Ref/Output is not resolvable by any ELBv2 read.
AWS: Smithy (elasticloadbalancingv2) DescribeLoadBalancers takes LoadBalancerArns (list) / Names and returns LoadBalancers; its only modeled error is LoadBalancerNotFoundException (awsQueryError code LoadBalancerNotFound, HTTP 400), whose message for this operation is the fixed sentence "One or more load balancers not found". Recorded AWS behavior: Ref for AWS::ElasticLoadBalancingV2::LoadBalancer is the load balancer's ARN, and once the stack is CREATE_COMPLETE that exact ARN resolves — DescribeLoadBalancers(LoadBalancerArns=[<Ref>]) returns exactly one LoadBalancer whose Type/Scheme/VpcId are the templated values ("network", "internal", the stack's VPC id) and whose State.Code is "active". The LoadBalancerNotFound refusal is correct only AFTER the stack is deleted — the same checks assert it there. A CREATE_COMPLETE stack whose resource is absent from the service that owns it is not a state AWS can be in.
Emulator: both checks get CREATE_COMPLETE from CloudFormation, read Outputs.NlbArn, and even see the API Gateway VPC link report that ARN in targetArns with status AVAILABLE — and then the first ELBv2 read of the same ARN is refused: HTTP 400 LoadBalancerNotFound, "One or more load balancers not found" (botocore.errorfactory.LoadBalancerNotFoundException raised from DescribeLoadBalancers). Either the CFN provider never writes the load balancer into ELBv2 state, or it publishes a Ref ARN ELBv2 does not own.
Why: a client that deploys an NLB by template and then drives it through the ELBv2 API — the standard IaC flow, and the only way to build the NLB an API Gateway VpcLink targets — gets a successful CREATE_COMPLETE followed by a not-found refusal for the resource the stack says it created. The refusal is also indistinguishable from the post-delete not-found the same operation must give, so a client cannot tell "created" from "deleted". Both checks abort at this call, so everything downstream (the NLB's Type/Scheme/VpcId/State readback, the VpcLink rollback reason "Failed to stabilize Vpc Link with id <id> Status Message NLB ARN not found." with HandlerErrorCode GeneralServiceException, and the post-delete not-found assertions) is never evaluated.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::LoadBalancer — provisioning must create the load balancer in ELBv2's state and publish that resource's real ARN as the physical id / Ref, so ELBv2 DescribeLoadBalancers (and DescribeListeners/DescribeTargetHealth) resolve it while the stack lives and only refuse it with LoadBalancerNotFound once it is deleted.
Traces: apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete; apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back.

## Evidence
Replay of suite simparity-behavioral-aws-v1: apigateway__cloudformation__ec2__elbv2 0/2 (0%), 2 failed; neither is marked ALREADY FILED or OWNED.

[error] apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-round-trip-and-delete:
  File "client.py", line 1094, in _make_api_call -> raise error_class(parsed_response, operation_name)
  botocore.errorfactory.LoadBalancerNotFoundException: An error occurred (LoadBalancerNotFound) when calling the DescribeLoadBalancers operation: One or more load balancers not found

[error] apigateway__cloudformation__ec2__elbv2.deploy-apigateway-vpclink-invalid-target-arn-rejected-and-rolled-back:
  identical frame, code and message, reached from the same helper on the control stack.

Both checks deploy one template (AWS::EC2::VPC, AWS::EC2::Subnet, an internal network AWS::ElasticLoadBalancingV2::LoadBalancer 'Nlb', and an AWS::ApiGateway::VpcLink whose TargetArns is [{"Ref": "Nlb"}]), assert the stack status is CREATE_COMPLETE (it is), read Outputs {VpcId, SubnetId, NlbArn: {Ref Nlb}, VpcLinkId}, and then call elbv2.describe_load_balancers(LoadBalancerArns=[NlbArn]) through the paginator.
Expected: exactly one LoadBalancer, with (Type, Scheme, VpcId, State.Code) == ("network", "internal", <the stack's VPC id>, "active").
Actual: HTTP 400 LoadBalancerNotFound "One or more load balancers not found" for the ARN the stack itself published. The fixed text of that message is the whole sentence (it names no value); the ARN, stack names, VPC/subnet ids and VPC link ids are per-run values.
The same operation and message ARE the correct answer later in the round-trip check, after delete_stack, so the emulator's refusal is right only for the post-delete leg and wrong while the stack is CREATE_COMPLETE.
Smithy consulted: elasticloadbalancingv2 DescribeLoadBalancers (input LoadBalancerArns/Names/Marker/PageSize 1..400; output LoadBalancers + NextMarker; error LoadBalancerNotFoundException, awsQueryError LoadBalancerNotFound, httpError 400).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both check ids in trace_ids reproduce.
2. cloudformation.create_stack with a template containing AWS::EC2::VPC (CidrBlock 10.93.0.0/16), AWS::EC2::Subnet (10.93.1.0/24 in the first AZ), an AWS::ElasticLoadBalancingV2::LoadBalancer 'Nlb' with Type=network, Scheme=internal, Subnets=[{Ref Subnet}], and an AWS::ApiGateway::VpcLink with TargetArns=[{Ref Nlb}]; Outputs VpcId, SubnetId, NlbArn={Ref Nlb}, VpcLinkId={Ref VpcLink}.
3. Poll DescribeStacks until CREATE_COMPLETE (it is reached) and read Outputs.NlbArn.
4. elbv2.describe_load_balancers(LoadBalancerArns=[<NlbArn>]): AWS returns the active internal network load balancer in the stack's VPC; the emulator answers HTTP 400 LoadBalancerNotFound "One or more load balancers not found".
5. delete_stack, then call the same operation again: only here should LoadBalancerNotFound be returned.
6. Re-replay the two check ids in trace_ids to confirm the fix.
