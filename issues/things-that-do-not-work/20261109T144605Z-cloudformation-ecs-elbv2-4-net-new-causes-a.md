Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive cloudformation__ecs__elbv2.cfn-ecs-service-health-check-grace-period-positive cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive
Issue-Key: elastic-load-balancing-v2|DescribeTargetGroupAttributes|TargetGroupNotFound
Issue-Key: cloudformation__ecs__elbv2.cfn-ecs-service-health-check-grace-period-positive|probe#13|IndexError|return ecs.describe_services(cluster=cluster, services=[service])["services"][0]
Issue-Key: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive|_poll_http_body#20|PublishedUrlRejected|_url, _route = ctx.endpoint_route(url)
Issue-Key: cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive|_poll_until#13|Timeout|last = probe()
Issue-Commit: 5252be52e0cc9f5d73bee5aa197523afb271707f

# cloudformation__ecs__elbv2: 4 net-new causes — a CFN target group is unknown to ELBv2, a CFN ECS service is unknown to ECS, the ALB's published DNSName is unservable, and the manual register/health workflows never settle

> _Note: this issue was identified against the code base at commit `5252be52e0cc9f5d73bee5aa197523afb271707f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 4: a target-group ARN exported by a CREATE_COMPLETE stack is not a target group ELBv2 knows.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetGroupAttributes takes a TargetGroupArn and returns its attributes (including deregistration_delay.timeout_seconds); TargetGroupNotFound is only for an ARN that does not exist. An ARN a successful stack exports does exist.
Emulator: `botocore.errorfactory.TargetGroupNotFoundException: An error occurred (TargetGroupNotFound) when calling the DescribeTargetGroupAttributes operation: Target groups 'arn:aws:elasticloadbalancing:<region>:<acct>:targetgroup/sim2real-TargetGro-<id>/<id>' not found`. The stack minted an ARN for a resource that was never created in ELBv2, so the attribute the template configured (DeregistrationDelay) cannot be read back.
Why: CREATE_COMPLETE is untrustworthy, and every ELBv2 call that uses a stack output fails with a not-found error that reads like a user mistake. The open issue 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md describes the same family; it does not carry this check.
Fix: the CloudFormation resource provider for AWS::ElasticLoadBalancingV2::TargetGroup must create the target group in ELBv2 state (including the attributes the template sets) or fail the stack.
Traces: cloudformation__ecs__elbv2.cfn-alb-target-group-deregistration-delay-configured-positive.

Cause 2 of 4: an ECS service created by a CREATE_COMPLETE stack is absent from ECS DescribeServices.
AWS: Smithy (ecs) DescribeServices returns a `services` list containing the named service (with healthCheckGracePeriodSeconds as declared); a service a successful stack created is always present.
Emulator: `IndexError: list index out of range` at `ecs.describe_services(cluster=cluster, services=[service])["services"][0]` — the services list comes back empty, so the healthCheckGracePeriodSeconds the template set can never be observed.
Why: a client cannot poll, update or scale the service its stack reports as created; the emulator returns an empty list rather than a ServiceNotFoundException, which hides even the diagnosis.
Fix: the CloudFormation resource provider for AWS::ECS::Service must register the service into ECS state so DescribeServices/ListServices see it.
Traces: cloudformation__ecs__elbv2.cfn-ecs-service-health-check-grace-period-positive.

Cause 3 of 4: the ALB's published DNSName is a real-AWS elb.amazonaws.com host the emulator does not serve.
AWS: Smithy (elasticloadbalancingv2) DescribeLoadBalancers returns LoadBalancer.DNSName — the address a client connects to; a GET through a path-based listener rule must reach the matching ECS service. The requirement is that the announced address is one the service answers on.
Emulator: `simparity.published.PublishedUrlRejected: endpoint_url was given http://sim2real-ecsalb-alb-<id>-<id>.us-west-2.elb.amazonaws.com/svc-a/ ...: its host is not one of the emulator's own hosts; a customer of the emulator needs http(s)://127.0.0.1:<port>/<resource path>, or http(s)://<own host>:<port>/<path> on a per-resource listener`.
Why: path-based routing cannot be exercised at all — the client is pointed at an address outside the emulator, so the ALB data plane is unusable.
Fix: the ELBv2 load-balancer serializer and its data-plane listener binding must publish the emulator's own host/port as DNSName.
Traces: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive. (The open issue 20261108T175930Z-cloudformation-ecs-elbv2-2-net-new-causes-elbv2.md covers neighbouring path-routing symptoms but not this check.)

Cause 4 of 4: the manual RegisterTargets/DeregisterTargets workflow and the health-check-path scenario never reach a terminal target-health state.
AWS: Smithy (elasticloadbalancingv2) RegisterTargets then DescribeTargetHealth moves a target through TargetHealth.State 'initial' to 'healthy' (or 'unhealthy' when the configured HealthCheckPath does not answer 200), and DeregisterTargets moves it to 'draining' and then out of the group. Each transition is observable in well under the checks' 180s/60s budgets.
Emulator: both checks are killed — `Error: Check timed out after 180.0s` and `Error: Check timed out after 60.0s` — with target health never reaching a terminal value, so neither the registration round trip nor the effect of HealthCheckPath can be observed.
Why: a client cannot tell whether its target registration worked or whether its health-check path is correct; the target-health state machine is effectively absent.
Fix: the ELBv2 target-health state machine — perform the configured health check against registered targets and drive TargetHealth.State through initial → healthy/unhealthy → draining on deregistration.
Traces: cloudformation__ecs__elbv2.cli-elbv2-register-unregister-targets-manual-workflow, cloudformation__ecs__elbv2.elbv2-target-health-check-path-affects-ecs-target-status-positive.

## Evidence
Cause 1: `TargetGroupNotFoundException: An error occurred (TargetGroupNotFound) when calling the DescribeTargetGroupAttributes operation: Target groups '<stack-exported arn>' not found`.
Cause 2: `IndexError: list index out of range` on `describe_services(...)["services"][0]` — empty services list for a CFN-created service.
Cause 3: `PublishedUrlRejected: endpoint_url was given http://sim2real-ecsalb-alb-<id>-<id>.us-west-2.elb.amazonaws.com/svc-a/ ... its host is not one of the emulator's own hosts`.
Cause 4: `Error: Check timed out after 180.0s` (register/unregister workflow) and `Error: Check timed out after 60.0s` (health-check-path), target health never terminal.

## Steps to Reproduce
Cause 1: CreateStack with AWS::ElasticLoadBalancingV2::TargetGroup (TargetGroupAttributes deregistration_delay.timeout_seconds); after CREATE_COMPLETE call elbv2 describe_target_group_attributes with the stack's output ARN.
Cause 2: CreateStack with AWS::ECS::Service (HealthCheckGracePeriodSeconds set); call ecs describe_services on the stack's cluster/service.
Cause 3: read DNSName from the stack's ALB and GET http://<DNSName>/svc-a/.
Cause 4: elbv2 create_target_group + register_targets, poll describe_target_health for 'healthy', then deregister_targets and poll for 'draining'.
