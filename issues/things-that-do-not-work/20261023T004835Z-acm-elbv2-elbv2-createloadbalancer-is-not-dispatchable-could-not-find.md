Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__elbv2.https-listener-without-a-certificate-rejected
Issue-Key: acm__elbv2|CreateLoadBalancer|InvalidAction
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# ELBv2 CreateLoadBalancer is not dispatchable: 'Could not find operation CreateLoadBalancer for version 2015-12-01' (InvalidAction)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
One root cause, grounded in one cross-service check (its id's leading token is the composite 'acm__elbv2'; the defect is in the Elastic Load Balancing v2 service, which the ACM check uses as its load-balancer prerequisite).

Cause 1 of 1: the ELBv2 request router does not know CreateLoadBalancer at the service's own API version, so the operation is unreachable.
AWS: Elastic Load Balancing v2 is an awsQuery service whose API version is 2015-12-01, and CreateLoadBalancer is bound to it — Smithy com.amazonaws.elasticloadbalancingv2 CreateLoadBalancer(Name required, Subnets, SubnetMappings, SecurityGroups, Scheme, Tags, Type, IpAddressType, ...) -> {LoadBalancers}, with modeled errors DuplicateLoadBalancerName, InvalidSubnet, InvalidScheme, InvalidSecurityGroup, TooManyLoadBalancers, etc. (all HTTP 400). A well-formed Action=CreateLoadBalancer&Version=2015-12-01 POST is served (200) or refused with one of those modeled errors; 'InvalidAction' is reserved for an action the service does not have.
Emulator: the call is refused with Error.Code 'InvalidAction' and message 'Could not find operation CreateLoadBalancer for version 2015-12-01' — the dispatcher resolved the version and found no such operation, so every load balancer creation (and everything downstream of it: listeners, target groups, certificate attachment) is impossible.
Why: a client cannot create a load balancer at all, and the failure masquerades as a client mistake ('InvalidAction' tells the caller it asked for an operation that does not exist) rather than a missing implementation. Every ACM/ELBv2 integration — HTTPS listener certificate attachment, InUseBy reporting, listener certificate rotation — is blocked behind it.
Fix: the operation dispatch/version table of the elasticloadbalancingv2 service crate (services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10) — register CreateLoadBalancer (and verify the rest of the awsQuery action table for API version 2015-12-01 resolves) so the request reaches the handler.
Traces: acm__elbv2.https-listener-without-a-certificate-rejected.

## Evidence
acm__elbv2.https-listener-without-a-certificate-rejected — botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the CreateLoadBalancer operation: Could not find operation CreateLoadBalancer for version 2015-12-01. Raised on the check's first ELBv2 call, before any ACM interaction, so nothing downstream was exercised.
Smithy citation: com.amazonaws.elasticloadbalancingv2#CreateLoadBalancer exists in the checked-out model (input CreateLoadBalancerInput, output CreateLoadBalancerOutput with LoadBalancers, awsQuery errors DuplicateLoadBalancerName/InvalidSubnet/InvalidScheme/...), i.e. the operation IS modeled for this service — only the dispatch is missing.
Note: the other five acm__elbv2 checks in this suite fail earlier, on the already-filed ACM ImportCertificate 501 stub (20261022T143245Z-acm-importcertificate-stub.md), so this is the only one that reached ELBv2.

## Steps to Reproduce
1. Boot the emulator.
2. elbv2 = boto3.client('elbv2'); elbv2.create_load_balancer(Name='sim2real-lb', Subnets=[<two subnet ids>], Type='application').
3. Observe HTTP error with Error.Code 'InvalidAction' and message 'Could not find operation CreateLoadBalancer for version 2015-12-01' instead of a 200 with LoadBalancers (or a modeled 400 such as InvalidSubnet).
4. Re-replay acm__elbv2.https-listener-without-a-certificate-rejected.
