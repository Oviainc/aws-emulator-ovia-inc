Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.modify-capacity-reservation-invalid-unit-count-rejected elbv2.modify-capacity-reservation-units-limit-exceeded-rejected
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.modify-capacity-reservation-invalid-unit-count-rejected|run#11|AssertionError|assert negative_code == "OperationNotPermitted", f"expected OperationNotPermitted, got {negative_code}"
Issue-Key: elbv2.modify-capacity-reservation-units-limit-exceeded-rejected|run#8|AssertionError|assert huge_code == "OperationNotPermitted", (huge_code, huge_message)
Issue-Commit: e8891ffb15bbac5be85198cac9e3766d8ad3529e

# elbv2 ModifyCapacityReservation applies range/quota validation instead of the account entitlement gate: -5 LCU answers ValidationError and 5,000,000 LCU answers CapacityUnitsLimitExceeded where AWS answers OperationNotPermitted for every value

> _Note: this issue was identified against the code base at commit `e8891ffb15bbac5be85198cac9e3766d8ad3529e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyCapacityReservation

## Description
ONE root cause, grounded in two failing checks.

Cause 1 of 1 — the entitlement gate that refuses every ModifyCapacityReservation on this account/region is missing; the handler answers a value-dependent error class instead.
AWS: Smithy ModifyCapacityReservation(LoadBalancerArn required, MinimumLoadBalancerCapacity, ResetCapacityReservation) models OperationNotPermittedException (client, httpError 400, awsQueryError code OperationNotPermitted) alongside CapacityUnitsLimitExceededException (CapacityUnitsLimitExceeded 400) and InvalidConfigurationRequestException (InvalidConfigurationRequest 400). The recordings show the entitlement check running FIRST and independently of the requested units: on a healthy, active internal ALB, MinimumLoadBalancerCapacity.CapacityUnits of -5, of the documented ALB floor 100, and of 5,000,000 are ALL refused identically with 400 OperationNotPermitted, and both checks require the two refusals on one load balancer to be byte-identical in code AND message. The message is account-level fixed text containing "does not support configuring minimum load balancer capacity reservation" (asserted as a substring, plus "does not support" and "capacity reservation"); it interpolates no request value — no unit count, no ceiling, no ARN. DescribeCapacityReservation afterwards reports no MinimumLoadBalancerCapacity and no CapacityReservationState, i.e. nothing is stored. AddTags/DescribeTags on the same load balancer succeed in the same run (the checks' positive control), so this is not a credential or load-balancer problem.
Emulator: the refusal is value-dependent and uses the wrong classes. CapacityUnits=-5 -> code ValidationError; CapacityUnits=5,000,000 -> code CapacityUnitsLimitExceeded with the emulator-authored message "You've exceeded the capacity units limit. At most 3000 capacity units can be reserved for a load balancer, and 5000000 were requested" (fixed text invented here, with the ceiling and the requested count interpolated). Neither refusal is the modeled OperationNotPermitted, the two refusals are not identical to each other, and no "does not support configuring minimum load balancer capacity reservation" sentence is ever produced.
Why: a client cannot distinguish "this account is not entitled to LCU pre-warming" (permanent, raise a support case / request the feature) from "your number was out of range" (retry with a smaller number) — it will retry a request that can never be accepted, and tooling that branches on OperationNotPermitted to fall back never takes that branch. The invented ceiling sentence also advertises a 3000-unit limit the service never states for this account.
Fix: the ModifyCapacityReservation handler in the elasticloadbalancingv2 service crate — evaluate the account/region entitlement for minimum-capacity reservation BEFORE any range or quota check on MinimumLoadBalancerCapacity.CapacityUnits, and return OperationNotPermitted with the service's account-level sentence; keep CapacityUnitsLimitExceeded/InvalidConfigurationRequest for an entitled account only, and leave the per-load-balancer capacity state untouched on refusal.
Not covered by an existing issue: open issue 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md Cause 7 records the OLD symptom (ModifyCapacityReservation answering 501 NotImplemented) and prescribes CapacityUnitsLimitExceeded for an above-quota request — which is precisely the behavior now observed and diverging. The operation now answers, so these two checks' signatures are new and the replay report does not mark them owned (only the sibling elbv2.capacity-reservation-exceeds-limit-rejected is).

## Evidence
Check elbv2.modify-capacity-reservation-invalid-unit-count-rejected: expected OperationNotPermitted for MinimumLoadBalancerCapacity={CapacityUnits:-5}; actual 'ValidationError'. Replay: 'assert negative_code == "OperationNotPermitted", ...' -> AssertionError: expected OperationNotPermitted, got ValidationError.
Check elbv2.modify-capacity-reservation-units-limit-exceeded-rejected: expected OperationNotPermitted with a message containing 'does not support configuring minimum load balancer capacity reservation' for MinimumLoadBalancerCapacity={CapacityUnits:5000000}; actual AssertionError: ('CapacityUnitsLimitExceeded', "You've exceeded the capacity units limit. At most 3000 capacity units can be reserved for a load balancer, and 5000000 were requested").
Both checks additionally require the 100-LCU request (the documented ALB floor) to be refused with the SAME code and SAME message as the out-of-range one, and DescribeCapacityReservation to report no MinimumLoadBalancerCapacity / no CapacityReservationState afterwards. The AddTags/DescribeTags positive control on the same load balancer passes.

## Steps to Reproduce
1. ec2 CreateVpc + two subnets; elbv2 CreateLoadBalancer(Type="application", Scheme="internal", Subnets=[...]) and wait for active.
2. elbv2 DescribeCapacityReservation(LoadBalancerArn) -> no MinimumLoadBalancerCapacity (baseline).
3. elbv2 ModifyCapacityReservation(LoadBalancerArn, MinimumLoadBalancerCapacity={CapacityUnits:5000000}) -> expect 400 OperationNotPermitted whose message contains 'does not support configuring minimum load balancer capacity reservation'; observe 400 CapacityUnitsLimitExceeded with the 3000-unit sentence.
4. elbv2 ModifyCapacityReservation(..., {CapacityUnits:-5}) -> expect the same 400 OperationNotPermitted; observe ValidationError.
5. elbv2 ModifyCapacityReservation(..., {CapacityUnits:100}) -> expect the identical 400 OperationNotPermitted code and message as step 3.
6. elbv2 DescribeCapacityReservation(LoadBalancerArn) -> expect no MinimumLoadBalancerCapacity and no CapacityReservationState.
