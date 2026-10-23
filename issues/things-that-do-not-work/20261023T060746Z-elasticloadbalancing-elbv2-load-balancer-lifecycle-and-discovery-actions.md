Issue-Source: trace-sweep-divergence
Issue-Trace: elasticloadbalancing-alb-lifecycle-register-targets
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: elasticloadbalancing-alb-lifecycle-register-targets elasticloadbalancing-delete-listener-idempotent-redelete elasticloadbalancing-listener-rule-path-based-routing elasticloadbalancing-modify-listener-port-updates elasticloadbalancing-modify-load-balancer-attributes-idle-timeout-round-trip elasticloadbalancing-rule-condition-values-mirroring elasticloadbalancing-set-security-groups-round-trip elbv2-capacity-reservation-default-state-on-new-load-balancer elbv2-describe-listeners-filtered-by-load-balancer elbv2-describe-rules-returns-priority-ordered elbv2-https-listener-with-cert elbv2-listener-attribute-and-certificate-error-paths elbv2-modify-listener-inplace-update-and-notfound elbv2-op-contract-describe-listeners elbv2env-https-listener-certificate-list elbv2env-listener-attributes-targets-and-rule-pagination elbv2err-deletion-protection-chain elbv2err-listener-create-rejections elbv2rule-action-enum-and-pattern-validation elbv2rule-condition-combination-limits elbv2rule-condition-legacy-values-mirroring elbv2rule-default-rule-immutable elbv2rule-priority-range-boundaries elbv2rule-redirect-component-pattern-validation elbv2rule-redirect-omitted-components-default-to-placeholders elbv2rule-redirect-self-loop-rejected elasticloadbalancing-elbv2-distinct-error-conditions elbv2err-create-load-balancer-input-rejections elbv2err-nlb-elastic-ip-rejections elbv2-load-balancer-listener-rule-lifecycle-error-conditions elbv2-loadbalancer-creation-subnet-validation-error elbv2env-alb-create-provisioning-and-subnet-reconfig elbv2-delete-nonexistent-load-balancer-is-idempotent elbv2-loadbalancer-describe-list-and-name-notfound elbv2-rule-target-group-load-balancer-not-found-errors elbv2-account-limits-and-ssl-policy-discovery
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->4xx|status
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|CreateLoadBalancer|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|CreateLoadBalancer|2xx->4xx|$.CreateLoadBalancerResponse
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|2xx->4xx|$.DeleteLoadBalancerResponse
Issue-Key: elasticloadbalancing|DeleteLoadBalancer|2xx->4xx|status
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->4xx|$.DescribeLoadBalancersResponse
Issue-Key: elasticloadbalancing|DescribeLoadBalancers|2xx->4xx|status
Issue-Key: elasticloadbalancing|DescribeAccountLimits|2xx->4xx|$.DescribeAccountLimitsResponse
Issue-Key: elasticloadbalancing|DescribeAccountLimits|2xx->4xx|status
Issue-Key: elasticloadbalancing|DescribeSSLPolicies|2xx->4xx|$.DescribeSSLPoliciesResponse
Issue-Key: elasticloadbalancing|DescribeSSLPolicies|2xx->4xx|status
Issue-Commit: c0919b0ae1fd753648687abf4b08014f282092d3

# elasticloadbalancing ELBv2 load-balancer lifecycle and discovery actions answer 400 InvalidAction

> _Note: this issue was identified against the code base at commit `c0919b0ae1fd753648687abf4b08014f282092d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoadBalancer

## Description
10 causes, all produced by ONE mechanism: no request for API version 2015-12-01 ever reaches an ELBv2 handler. The shared signing name / endpoint prefix 'elasticloadbalancing' is claimed by the Classic ELB crate, whose front door (services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service/api_version.rs: serves_requested_version accepts only CLASSIC_API_VERSION '2012-06-01', everything else goes to UnsupportedApiVersionOperation) answers 400 InvalidAction 'Could not find operation <Action> for version <Version>'. The ELBv2 model (com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10, 2015-12-01, awsQuery, 51 operations) is reachable only under its own route key 'elasticloadbalancingv2' (elasticloadbalancingv2 crate src/service.rs ROUTE_KEY), which no AWS SDK/CLI client uses. Related OPEN issues that do NOT cover these fixture steps and carry no wire replay key: 20261023T025808Z-elbv2-awsquery-action-table-at-api-version-2015.md (behavioral check ids only), 20261023T004835Z-acm-elbv2-elbv2-createloadbalancer-is-not-dispatchable-could-not-find.md, 20261022T220446Z-elbv2-elastic-load-balancing-v2-is-unreachable-every-request.md (AmbiguousService symptom). Every cause below needs the version dispatch fixed FIRST, and then the named operation's own handler/validation/serializer in the elasticloadbalancingv2 crate; a fix that only routes the action leaves the per-operation behavior below unverified.

Cause 1 of 10: CreateLoadBalancer success path.
AWS: Smithy CreateLoadBalancer(Name required, Subnets/SubnetMappings, SecurityGroups, Scheme, Type, IpAddressType, Tags) -> output {LoadBalancers}; a valid ALB create is answered 200 with LoadBalancerArn, DNSName, CanonicalHostedZoneId, AvailabilityZones, Type/Scheme/IpAddressType and State.Code 'provisioning'.
Emulator: 400 InvalidAction, no CreateLoadBalancerResponse.
Why: nothing in ELBv2 can be created, so every listener/rule/target workflow built on a load balancer is blocked at its first step.
Fix: version dispatch, then the CreateLoadBalancer handler, state and XML serializer.
Traces: elasticloadbalancing-alb-lifecycle-register-targets#setup#1 (cluster of 26 setup steps).

Cause 2 of 10: CreateLoadBalancer duplicate-name refusal.
AWS: Smithy declares DuplicateLoadBalancerNameException, awsQueryError 'DuplicateLoadBalancerName', HTTP 400; re-creating an existing name with different settings is refused with that code and a request-derived message naming the name (recorded example: "A load balancer with the same name 'sim2real-probe-elbv2-a-2c1778a7' exists, but with different settings").
Emulator: 400 Code 'InvalidAction'.
Why: an idempotency/conflict branch keyed on DuplicateLoadBalancerName never fires.
Fix: version dispatch, then CreateLoadBalancer's name-uniqueness check and modeled-error rendering.
Traces: elasticloadbalancing-alb-lifecycle-register-targets#task#9 (cluster of 8).

Cause 3 of 10: CreateLoadBalancer unknown-subnet refusal.
AWS: Smithy declares SubnetNotFoundException, awsQueryError 'SubnetNotFound', HTTP 400 (recorded message example 'InvalidSubnetID.NotFound').
Emulator: 400 Code 'InvalidAction'.
Why: a client cannot tell a bad subnet from a nonexistent API.
Fix: version dispatch, then CreateLoadBalancer subnet validation against EC2 state.
Traces: elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#3 (cluster of 2).

Cause 4 of 10: CreateLoadBalancer internal-scheme provisioning response.
AWS: 200 with Scheme 'internal', the requested AvailabilityZones/SubnetIds, VpcId, CanonicalHostedZoneId and State.Code 'provisioning'.
Emulator: 400 InvalidAction, response missing.
Why: a provisioning-then-poll workflow never starts.
Fix: version dispatch, then CreateLoadBalancer state modelling of the provisioning transition.
Traces: elbv2env-alb-create-provisioning-and-subnet-reconfig#task#5 (cluster of 3).

Cause 5 of 10: DeleteLoadBalancer on an absent ARN.
AWS: Smithy DeleteLoadBalancer -> empty output; AWS treats deleting an already-absent load balancer as success, answering 200 with <DeleteLoadBalancerResult/> (LoadBalancerNotFound is modeled but not raised on this idempotent path per the recording).
Emulator: 400 InvalidAction.
Why: teardown code that deletes unconditionally fails instead of being a no-op.
Fix: version dispatch, then DeleteLoadBalancer's idempotent-delete handler.
Traces: elbv2-delete-nonexistent-load-balancer-is-idempotent#task#1.

Cause 6 of 10: DescribeLoadBalancers not-found by name.
AWS: Smithy declares LoadBalancerNotFoundException, awsQueryError 'LoadBalancerNotFound', HTTP 400, with a request-derived message naming the queried names (example "Load balancers '[sim2real-probe-absent-lb]' not found").
Emulator: 400 Code 'InvalidAction'.
Why: a client cannot distinguish absence from a dead API.
Fix: version dispatch, then DescribeLoadBalancers' Names lookup and error rendering.
Traces: elasticloadbalancing-elbv2-distinct-error-conditions#task#2.

Cause 7 of 10: DescribeLoadBalancers not-found by name (second recording, same code, different name).
AWS: as Cause 6, LoadBalancerNotFound / 400.
Emulator: 400 InvalidAction.
Why: same.
Fix: same handler.
Traces: elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#4 (cluster of 3).

Cause 8 of 10: DescribeLoadBalancers empty-result success path.
AWS: an unfiltered DescribeLoadBalancers with nothing provisioned is 200 with an empty LoadBalancers element, not an error.
Emulator: 400 InvalidAction.
Why: list-then-act code cannot even observe an empty account.
Fix: version dispatch, then DescribeLoadBalancers' list serializer for the empty case.
Traces: elbv2-loadbalancer-describe-list-and-name-notfound#task#1.

Cause 9 of 10: DescribeAccountLimits.
AWS: Smithy DescribeAccountLimits -> {Limits, NextMarker}, NO modeled errors, so any error is a divergence; 200 with the limit catalogue (target-groups 3000, listeners-per-application-load-balancer 50, rules-per-application-load-balancer 100, trust-stores 20, ...).
Emulator: 400 InvalidAction.
Why: quota-aware clients cannot read any limit.
Fix: version dispatch, then DescribeAccountLimits' seeded limit catalogue.
Traces: elbv2-account-limits-and-ssl-policy-discovery#task#1 (cluster of 2).

Cause 10 of 10: DescribeSSLPolicies success path.
AWS: Smithy DescribeSSLPolicies -> {SslPolicies, NextMarker}; 200 with the predefined policies (Name, Ciphers[Name,Priority], SslProtocols, SupportedLoadBalancerTypes), e.g. ELBSecurityPolicy-2016-08 and the TLS13 families.
Emulator: 400 InvalidAction.
Why: an HTTPS listener cannot be configured against a discoverable policy name.
Fix: version dispatch, then DescribeSSLPolicies' catalogue.
Traces: elbv2-account-limits-and-ssl-policy-discovery#task#2 (cluster of 3).

## Evidence
Every actual response below is the Classic front door's UnsupportedApiVersionOperation envelope: HTTP 400, $.ErrorResponse.Error.Code 'InvalidAction', Message 'Could not find operation <Action> for version 2015-12-01', and no $.ErrorResponse.RequestId.
- elasticloadbalancing-alb-lifecycle-register-targets#setup#1 CreateLoadBalancer: expected 200, got 400 (26 steps in cluster).
- elasticloadbalancing-alb-lifecycle-register-targets#task#9 CreateLoadBalancer: expected Code 'DuplicateLoadBalancerName'; got 'InvalidAction' (8 steps).
- elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#3 CreateLoadBalancer: expected Code 'SubnetNotFound' (message example 'InvalidSubnetID.NotFound'); got 'InvalidAction'.
- elbv2env-alb-create-provisioning-and-subnet-reconfig#task#5 CreateLoadBalancer: expected 200 + CreateLoadBalancerResult.LoadBalancers.member{Scheme 'internal', State.Code 'provisioning', CanonicalHostedZoneId 'Z1H1FL5HABSF5'}; got 400.
- elbv2-delete-nonexistent-load-balancer-is-idempotent#task#1 DeleteLoadBalancer: expected 200 + DeleteLoadBalancerResult; got 400.
- elasticloadbalancing-elbv2-distinct-error-conditions#task#2 / elbv2-load-balancer-listener-rule-lifecycle-error-conditions#task#4 DescribeLoadBalancers: expected Code 'LoadBalancerNotFound'; got 'InvalidAction'.
- elbv2-loadbalancer-describe-list-and-name-notfound#task#1 DescribeLoadBalancers: expected 200 + empty LoadBalancers; got 400.
- elbv2-account-limits-and-ssl-policy-discovery#task#1 DescribeAccountLimits and #task#2 DescribeSSLPolicies: expected 200 + Limits / SslPolicies; got 400.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Sign for service 'elasticloadbalancing' (as an AWS SDK elbv2 client does) and POST 'Action=CreateLoadBalancer&Version=2015-12-01&Name=probe-lb&Subnets.member.1=<subnet>&Type=application'. Observe 400 InvalidAction 'Could not find operation CreateLoadBalancer for version 2015-12-01' instead of 200 + LoadBalancers.
3. Repeat with Action=DeleteLoadBalancer (absent ARN), DescribeLoadBalancers (no filter, and Names.member.1=absent-lb), DescribeAccountLimits and DescribeSSLPolicies at Version=2015-12-01 — all answer the same refusal.
4. Re-replay the fixture steps named in issue_step and trace_ids.
