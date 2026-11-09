Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected
Issue-Key: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected|run#6|AssertionError|assert status == "ROLLBACK_COMPLETE", f"invalid-scheme stack ended {status!r}"
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudformation__elbv2: a stack whose load balancer declares an invalid Scheme reaches CREATE_COMPLETE instead of failing the resource with InvalidScheme

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One net-new root cause (Cause 1 of 1). Related open filings that do NOT carry this replay key: 20261107T201046Z-cloudformation-elbv2-a-stack-whose-two-aws-elasticloadbalancingv2.md and 20261023T025808Z-cloudformation-elbv2-a-cloudformation-stack-whose-elbv2-listener-rules-collide.md cover the rule-priority collision variant of "the stack does not roll back", not the Scheme validation; the suite does not mark this check OWNED.

Cause 1 of 1: an AWS::ElasticLoadBalancingV2::LoadBalancer whose Scheme is not a modeled value is accepted, and the stack reaches CREATE_COMPLETE instead of failing the resource and rolling back.
AWS: Smithy (elasticloadbalancingv2) CreateLoadBalancer.Scheme targets the LoadBalancerSchemeEnum (internet-facing | internal), and the operation declares InvalidSchemeException with awsQueryError code InvalidScheme, HTTP 400 — the error AWS raises for a Scheme outside that enum. CloudFormation surfaces that refusal as a CREATE_FAILED resource whose ResourceStatusReason carries the service error, and the stack ends ROLLBACK_COMPLETE.
Emulator: the stack with the invalid Scheme ends 'CREATE_COMPLETE' — the underlying CreateLoadBalancer accepted a value outside LoadBalancerSchemeEnum rather than refusing it with InvalidScheme, and nothing failed or rolled back.
Why: a client gets a green stack for a load balancer that cannot exist as described, so the scheme it believes it deployed (and everything that depends on it — public resolvability, subnet requirements, internet gateway requirements) is silently different from what the template asked for. Template validation in CI, which keys on the stack reaching a failed state, passes.
Fix: the ELBv2 CreateLoadBalancer handler's input validation must reject a Scheme outside LoadBalancerSchemeEnum with InvalidScheme (HTTP 400), and the CloudFormation AWS::ElasticLoadBalancingV2::LoadBalancer provider must propagate that refusal as a failed resource so the stack rolls back.
Traces: cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected (listed by the suite under the cloudformation__elbv2 heading).

## Evidence
Replay of suite simparity-behavioral-aws-v1: service heading cloudformation__elbv2 0/2 (0%), 2 failed; one is OWNED, this one is not.

[mismatch] cloudformation__cloudfront__ec2__elbv2.deploy-loadbalancer-invalid-scheme-rejected (under ### cloudformation__elbv2)
  AssertionError: invalid-scheme stack ended 'CREATE_COMPLETE'

Expected: the AWS::ElasticLoadBalancingV2::LoadBalancer resource fails with the service's InvalidScheme (HTTP 400) refusal and the stack ends ROLLBACK_COMPLETE. Actual: CREATE_COMPLETE.

Smithy consulted: elasticloadbalancingv2 CreateLoadBalancer — input Scheme -> LoadBalancerSchemeEnum (internet-facing | internal); errors include InvalidSchemeException (awsQueryError code InvalidScheme, httpError 400).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the id in trace_ids reproduces.
2. cloudformation.create_stack with a template whose AWS::ElasticLoadBalancingV2::LoadBalancer sets Scheme to a value outside internet-facing/internal.
3. Poll describe_stacks for a terminal status. Expect ROLLBACK_COMPLETE with the load-balancer resource CREATE_FAILED and an InvalidScheme reason; the emulator reaches CREATE_COMPLETE.
4. Call elbv2.create_load_balancer directly with the same out-of-enum Scheme: expect HTTP 400 InvalidScheme; the emulator accepts it.
5. Re-replay the id in trace_ids to confirm the fix.
