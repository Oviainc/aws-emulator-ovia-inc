Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.add-listener-certificate-nonexistent-arn-rejected elbv2.add-listener-certificates-unknown-cert-fails elbv2.create-listener-unknown-ssl-policy-name-rejected elbv2.create-rule-condition-value-exceeds-length-limit-rejected elbv2.modify-target-group-invalid-health-check-fails elbv2.target-group-attribute-legality-by-protocol-rejected elbv2.set-security-groups-empty-list-not-silently-accepted elbv2.modify-ip-pools-nonexistent-lb-fails elbv2.modify-ip-pools-updates-lb-attributes elbv2.create-load-balancer-missing-permission-denied elbv2.create-target-group-identical-params-is-idempotent elbv2.concurrent-create-listener-same-port-race
Issue-Shape-Id: com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10
Issue-Key: elbv2.add-listener-certificate-nonexistent-arn-rejected|_error_code#10|AssertionError|raise AssertionError(
Issue-Key: elbv2.add-listener-certificates-unknown-cert-fails|_error_code#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: elbv2.create-listener-unknown-ssl-policy-name-rejected|_error_code#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: elbv2.create-rule-condition-value-exceeds-length-limit-rejected|_error#14|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: elbv2.modify-target-group-invalid-health-check-fails|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: elbv2.target-group-attribute-legality-by-protocol-rejected|_error_code#21|AssertionError|raise AssertionError(
Issue-Key: elbv2.set-security-groups-empty-list-not-silently-accepted|_error#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: elbv2.modify-ip-pools-nonexistent-lb-fails|_error#4|AssertionError|raise AssertionError("expected a ClientError, call unexpectedly succeeded")
Issue-Key: elbv2.modify-ip-pools-updates-lb-attributes|run#14|AssertionError|raise AssertionError(
Issue-Key: elbv2.create-load-balancer-missing-permission-denied|_error#16|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: elbv2|CreateTargetGroup|DuplicateTargetGroupName
Issue-Key: elbv2.concurrent-create-listener-same-port-race|run#9|AssertionError|assert len(successes) == 1, f"expected exactly one winning CreateListener, got {outcomes}"
Issue-Commit: 7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae

# elbv2: ten overly-permissive control-plane gaps — calls real ELBv2 refuses are answered 200 OK (certs, SSL policy, rule limits, health-check params, SGs, IP pools, idempotency)

> _Note: this issue was identified against the code base at commit `7fd8acd3be042b6d00a1996e0a0d05df0b6f87ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddListenerCertificates

## Description
Ten net-new root causes, all of the same family: the emulator's ELBv2 control plane accepts a request that real ELBv2 refuses (or refuses one it accepts). Each cause is independent and must be closed separately.

Cause 1 of 10: AddListenerCertificates does not check that the certificate exists.
AWS: Smithy: AddListenerCertificates errors include CertificateNotFoundException (awsQueryError code `CertificateNotFound`, HTTP 400). A certificate ARN that names no certificate in the account/region is refused; nothing is added to the listener's certificate list.
Emulator: returns HTTP 200 with `Certificates: [{CertificateArn: <the bogus arn>, IsDefault: false}]` for both an ARN in this account that was never imported and an ARN in a foreign account (111122223333).
Why: a client that relies on CertificateNotFound to detect a typo'd/deleted certificate silently builds an HTTPS listener whose SNI certificate list references a certificate that cannot be served; the failure surfaces only in the data plane.
Fix: the AddListenerCertificates handler's validation — resolve each CertificateArn against ACM/IAM certificate state before mutating the listener certificate list.
Traces: elbv2.add-listener-certificate-nonexistent-arn-rejected, elbv2.add-listener-certificates-unknown-cert-fails.

Cause 2 of 10: CreateListener accepts an unknown SslPolicy name.
AWS: Smithy: CreateListener errors include SSLPolicyNotFoundException (awsQueryError code `SSLPolicyNotFound`, HTTP 400). Only the named ELBSecurityPolicy-* policies are accepted.
Emulator: returns HTTP 200 and stores `SslPolicy: 'ELBSecurityPolicy-Nonexistent-9999'` verbatim on the listener.
Why: a client cannot detect a misspelled or region-unsupported security policy, and the listener reports a TLS policy that bounds nothing.
Fix: the CreateListener handler's validation — reject SslPolicy values outside the supported policy table with SSLPolicyNotFound.
Traces: elbv2.create-listener-unknown-ssl-policy-name-rejected.

Cause 3 of 10: CreateRule does not enforce the per-condition value length limit.
AWS: recorded behavior only for the exact limit; AWS refuses a rule condition value longer than the documented maximum with a 400 ValidationError naming the member, and the rule is not created.
Emulator: the call succeeds (the check reports "expected the call to be refused, but it succeeded") and the over-long condition value is stored on the rule.
Why: an over-long condition is accepted at create time and can never match, so routing silently falls through to the default action instead of being rejected at configuration time.
Fix: the CreateRule handler's validation of RuleCondition values (length/count bounds).
Traces: elbv2.create-rule-condition-value-exceeds-length-limit-rejected.

Cause 4 of 10: ModifyTargetGroup accepts a health-check interval below the health-check timeout.
AWS: Smithy: ModifyTargetGroup declares InvalidConfigurationRequestException (code `InvalidConfigurationRequest`, HTTP 400); HealthCheckIntervalSeconds @range 5-300 and HealthCheckTimeoutSeconds @range 2-120. AWS additionally refuses the combination where the interval is not greater than the timeout.
Emulator: returns HTTP 200 with the target group showing `HealthCheckIntervalSeconds: 5, HealthCheckTimeoutSeconds: 30` — an interval shorter than the timeout.
Why: a client gets a target group whose health-check schedule is impossible; on real AWS the same template is rejected, so a deployment that works here fails in production.
Fix: the ModifyTargetGroup (and CreateTargetGroup) handler's cross-member health-check validation.
Traces: elbv2.modify-target-group-invalid-health-check-fails.

Cause 5 of 10: ModifyTargetGroupAttributes accepts load_balancing.algorithm.type=least_outstanding_requests together with a non-zero slow_start.duration_seconds.
AWS: recorded behavior only: AWS refuses this combination with a 400 InvalidConfigurationRequest; slow start is supported only with round_robin.
Emulator: returns HTTP 200 and the attribute list comes back with both `load_balancing.algorithm.type=least_outstanding_requests` and `slow_start.duration_seconds=60`.
Why: a configuration AWS rejects is accepted, so the emulator cannot be used to validate target-group attribute legality, and the two mutually exclusive attributes are both reported as in effect.
Fix: the ModifyTargetGroupAttributes handler's attribute-legality validation.
Traces: elbv2.target-group-attribute-legality-by-protocol-rejected.

Cause 6 of 10: SetSecurityGroups accepts an empty SecurityGroups list on an ALB.
AWS: recorded behavior only: an application load balancer must keep at least one security group; AWS refuses the empty list with a 400 error and leaves the existing groups in place.
Emulator: returns HTTP 200 with `SecurityGroupIds: []`, leaving the load balancer with no security group at all.
Why: the call silently strips all ingress control from the load balancer — the opposite of the AWS guarantee a client depends on.
Fix: the SetSecurityGroups handler's validation (reject an empty list for type=application).
Traces: elbv2.set-security-groups-empty-list-not-silently-accepted.

Cause 7 of 10: ModifyIpPools validates neither the load balancer nor the IPAM pool.
AWS: recorded behavior only: ModifyIpPools refuses an unknown LoadBalancerArn with LoadBalancerNotFound (HTTP 400) and refuses an IPAM pool that has no provisioned CIDR with a 400 validation error.
Emulator: both calls succeed — a nonexistent load balancer ARN returns success instead of a ClientError, and a pool with no provisioned CIDR is accepted and recorded on the load balancer's attributes.
Why: a client cannot distinguish a typo'd load balancer or an unusable IPAM pool from a successful configuration change; the load balancer then reports an IP pool it cannot allocate from.
Fix: the ModifyIpPools handler — resolve the load balancer ARN and the IPAM pool state before mutating attributes.
Traces: elbv2.modify-ip-pools-nonexistent-lb-fails, elbv2.modify-ip-pools-updates-lb-attributes.

Cause 8 of 10: CreateLoadBalancer is not authorization-gated.
AWS: recorded behavior only: a principal without elasticloadbalancing:CreateLoadBalancer is refused with HTTP 403 AccessDenied and no load balancer is created.
Emulator: the call succeeds ("expected the call to be refused, but it succeeded") and the load balancer exists.
Why: negative/permission tests pass against the emulator and fail against AWS; least-privilege policies cannot be validated at all.
Fix: the CreateLoadBalancer handler's authorization check (request principal vs. the attached policy), not its parameter validation.
Traces: elbv2.create-load-balancer-missing-permission-denied.

Cause 9 of 10: CreateTargetGroup with identical parameters is not idempotent.
AWS: Smithy: CreateTargetGroup declares DuplicateTargetGroupNameException (code `DuplicateTargetGroupName`, HTTP 400). AWS raises it only when the name is reused with DIFFERENT settings; a repeat call with the identical parameter set returns the existing target group with HTTP 200.
Emulator: raises DuplicateTargetGroupName with the message "A target group with the same name '<name>' exists, but with different settings" even though every parameter is identical.
Why: idempotent retry paths (SDK retries, CFN/IaC re-apply) break: the second attempt errors instead of converging on the existing target group, and the error message asserts a settings difference that does not exist.
Fix: the CreateTargetGroup handler's duplicate-name branch — compare the full stored setting set and return the existing group when it matches.
Traces: elbv2.create-target-group-identical-params-is-idempotent.

Cause 10 of 10: concurrent CreateListener calls on the same port both succeed and return the same listener ARN.
AWS: Smithy: CreateListener declares DuplicateListenerException (code `DuplicateListener`, HTTP 400). Exactly one of two racing creates on the same (load balancer, port) wins; the other is refused with DuplicateListener.
Emulator: both calls return ('ok', <same listener ARN>) — the check reports "expected exactly one winning CreateListener, got [('ok', arn...), ('ok', arn...)]" with an identical ARN in both slots.
Why: a client that uses CreateListener as a create-once primitive under concurrency sees two successes and cannot tell which one owns the listener; the duplicate-port invariant is unenforced.
Fix: the CreateListener handler's port-uniqueness check must be applied under the same state lock as the insert.
Traces: elbv2.concurrent-create-listener-same-port-race.

## Evidence
All quotes are from the behavioral replay output.

Cause 1: `AssertionError: expected AddListenerCertificates to reject the bogus certificate ARN, but it succeeded: {'Certificates': [{'CertificateArn': 'arn:aws:acm:us-west-2:000000000000:certificate/b451d90b-...-745e00000000', 'IsDefault': False}], ... 'HTTPStatusCode': 200}`. Expected: HTTP 400, code CertificateNotFound. Second check: same 200 for `arn:aws:acm:us-west-2:111122223333:certificate/00000000-...`.
Cause 2: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Listeners': [{... 'SslPolicy': 'ELBSecurityPolicy-Nonexistent-9999' ...}], 'HTTPStatusCode': 200}`. Expected: HTTP 400, code SSLPolicyNotFound.
Cause 3: `AssertionError: expected the call to be refused, but it succeeded`. Expected: HTTP 400 ValidationError naming the condition member.
Cause 4: `AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'TargetGroups': [{... 'HealthCheckIntervalSeconds': 5, 'HealthCheckTimeoutSeconds': 30 ...}], 'HTTPStatusCode': 200}`. Expected: HTTP 400 InvalidConfigurationRequest.
Cause 5: `AssertionError: least_outstanding_requests while slow start is enabled: expected the call to be rejected, but it SUCCEEDED and returned {'Attributes': [... {'Key': 'load_balancing.algorithm.type', 'Value': 'least_outstanding_requests'}, {'Key': 'slow_start.duration_seconds', 'Value': '60'} ...]}`.
Cause 6: `AssertionError: expected the call to be rejected, but it succeeded: {'SecurityGroupIds': [], ... 'HTTPStatusCode': 200}`.
Cause 7: `AssertionError: expected a ClientError, call unexpectedly succeeded` (nonexistent LB) and `AssertionError: expected ModifyIpPools to refuse pool ipam-pool-... (no provisioned CIDR), but it returned success`.
Cause 8: `AssertionError: expected the call to be refused, but it succeeded`. Expected: HTTP 403 AccessDenied.
Cause 9: `botocore.errorfactory.DuplicateTargetGroupNameException: An error occurred (DuplicateTargetGroupName) when calling the CreateTargetGroup operation: A target group with the same name '<tg-name>' exists, but with different settings` — raised for an identical-parameter repeat. Expected: HTTP 200 returning the existing target group.
Cause 10: `AssertionError: expected exactly one winning CreateListener, got [('ok', 'arn:...:listener/app/<lb>/<id>/<listener-id>'), ('ok', 'arn:...:listener/app/<lb>/<id>/<listener-id>')]` — the same ARN twice. Expected: one 200 and one HTTP 400 DuplicateListener.

The ARNs, names, ids, request ids and dates above are per-run values from one replay; the requirement is the status/code/behavior, not those values.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids:
1. elbv2.add-listener-certificate-nonexistent-arn-rejected / elbv2.add-listener-certificates-unknown-cert-fails — create an internal ALB + HTTPS listener with a real default certificate, then AddListenerCertificates with a certificate ARN that was never imported (and one in a foreign account); expect 400 CertificateNotFound.
2. elbv2.create-listener-unknown-ssl-policy-name-rejected — CreateListener Protocol=HTTPS SslPolicy='ELBSecurityPolicy-Nonexistent-9999'; expect 400 SSLPolicyNotFound.
3. elbv2.create-rule-condition-value-exceeds-length-limit-rejected — CreateRule with a condition value above the documented length cap; expect a 400 refusal.
4. elbv2.modify-target-group-invalid-health-check-fails — ModifyTargetGroup with HealthCheckIntervalSeconds=5 and HealthCheckTimeoutSeconds=30; expect 400 InvalidConfigurationRequest.
5. elbv2.target-group-attribute-legality-by-protocol-rejected — ModifyTargetGroupAttributes with load_balancing.algorithm.type=least_outstanding_requests and slow_start.duration_seconds=60; expect a 400 refusal.
6. elbv2.set-security-groups-empty-list-not-silently-accepted — SetSecurityGroups with SecurityGroups=[] on an ALB; expect a 400 refusal.
7. elbv2.modify-ip-pools-nonexistent-lb-fails / elbv2.modify-ip-pools-updates-lb-attributes — ModifyIpPools against an unknown LoadBalancerArn, and against an IPAM pool with no provisioned CIDR; expect refusals.
8. elbv2.create-load-balancer-missing-permission-denied — CreateLoadBalancer as a principal lacking the permission; expect 403 AccessDenied.
9. elbv2.create-target-group-identical-params-is-idempotent — CreateTargetGroup twice with byte-identical parameters; expect the second to return 200 with the existing group.
10. elbv2.concurrent-create-listener-same-port-race — issue two CreateListener calls concurrently for the same load balancer and port; expect exactly one 200 and one 400 DuplicateListener.
