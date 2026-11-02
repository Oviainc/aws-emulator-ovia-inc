Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elbv2.add-tags-duplicate-keys-rejected elbv2.add-tags-nonexistent-resource-fails elbv2.duplicate-tag-key-in-single-request-rejected elbv2.create-load-balancer-empty-vs-absent-tags-preserved elbv2.empty-string-vs-absent-tag-value-distinguished elbv2.remove-tags-then-verify-and-reject-missing-resource elbv2.tag-value-unicode-and-special-characters-round-trip elbv2.tag-value-unicode-round-trip elbv2.target-group-tag-unicode-value-round-trip elbv2.unicode-tag-value-round-trip elbv2.modify-capacity-reservation-invalid-unit-count-rejected elbv2.modify-capacity-reservation-units-out-of-range-rejected elbv2__resourcegroupstaggingapi.load-balancer-and-target-group-tags-visible-through-get-resources autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule elbv2.create-trust-store-reaches-active elbv2.create-trust-store-duplicate-name-fails elbv2.create-trust-store-invalid-ca-bundle-rejected elbv2.trust-store-ca-bundle-server-side-validation-on-create elbv2.trust-store-invalid-bundle-rejected elbv2.trust-store-bucket-not-found-error elbv2.trust-store-not-found-family elbv2.trust-store-ops-not-found elbv2.trust-store-lifecycle elbv2.trust-store-lifecycle-create-active-delete-404 elbv2.trust-store-lifecycle-create-modify-delete elbv2.trust-store-create-active-delete-404 elbv2.trust-store-creation-state-transition elbv2.trust-store-name-dedup elbv2.trust-store-modify-updates-bundle elbv2.trust-store-in-use-delete-fails elbv2.trust-store-association-lifecycle elbv2.trust-store-association-and-shared-deletion-rejected elbv2.trust-store-ca-bundle-retrieval elbv2.trust-store-ca-bundle-binary-content-byte-equality elbv2.trust-store-revocation-lifecycle elbv2.trust-store-revocation-add-describe-remove elbv2.trust-store-revocation-add-describe-remove-lifecycle elbv2.trust-store-revocation-add-describe-remove-roundtrip elbv2.add-trust-store-revocation-invalid-content-rejected elbv2.add-trust-store-revocations-invalid-content-fails elbv2.add-trust-store-revocations-malformed-crl-rejected elbv2.add-trust-store-revocations-too-many-entries-fails elbv2.remove-trust-store-revocations-unknown-id-not-silently-accepted elbv2.delete-trust-store-in-use-rejected elbv2.delete-trust-store-in-use-by-listener-fails elbv2.delete-trust-store-then-not-found elbv2.describe-trust-store-associations-for-shared-listener elbv2.modify-trust-store-updates-ca-bundle elbv2.modify-trust-store-invalid-bundle-fails elbv2.modify-trust-store-missing-bundle-rejected elbv2.modify-trust-store-nonexistent-bundle-rejected elbv2.get-trust-store-ca-bundle-byte-equality elbv2.get-trust-store-ca-bundle-byte-equality-round-trip elbv2.get-trust-store-ca-bundle-byte-fidelity elbv2.get-trust-store-ca-bundle-returns-full-uploaded-bundle elbv2.get-trust-store-ca-bundle-unknown-truststore-fails elbv2.get-trust-store-revocation-content-byte-equality elbv2.get-trust-store-revocation-content-byte-equality-round-trip elbv2.get-trust-store-revocation-content-byte-fidelity elbv2.get-trust-store-revocation-content-matches-added elbv2.get-trust-store-revocation-content-returns-full-crl-bytes elbv2.get-trust-store-revocation-content-unknown-id-fails elbv2.get-resource-policy-consistent-read elbv2.get-resource-policy-no-policy-rejected elbv2.get-resource-policy-nonexistent-fails elbv2.get-resource-policy-returns-attached-json-policy elbv2.get-resource-policy-returns-matching-policy-document elbv2.resource-policy-not-found elbv2.e2e-trust-store-mtls-association-and-revocation-flow elbv2.delete-target-group-in-use-fails elbv2.e2e-target-group-deletion-while-referenced-rejected cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes elbv2.listener-default-action-empty-vs-absent-fields-preserved elbv2.modify-rule-updates-actions-and-conditions elbv2.set-rule-priorities-ignores-other-listener elbv2.network-config-error-semantics elbv2.set-subnets-mismatched-vpc-rejected elbv2.capacity-reservation-exceeds-limit-rejected elbv2.server-side-capacity-reservation-minimum-units-floor-enforced elbv2.op-contract-describe-account-limits elbv2.describe-ssl-policies-unknown-name-rejected elbv2.describe-ssl-policies-malformed-name-filter-rejected elbv2.describe-ssl-policies-malformed-name-format-rejected elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected
Issue-Key: elbv2.add-tags-duplicate-keys-rejected|run#5|AssertionError|assert code == "DuplicateTagKeys", f"expected DuplicateTagKeys, got {code!r}"
Issue-Key: elbv2|AddTags|NotImplemented
Issue-Key: elbv2.duplicate-tag-key-in-single-request-rejected|run#7|AssertionError|assert code == "DuplicateTagKeys", f"expected DuplicateTagKeys, got {code!r}"
Issue-Key: elbv2__resourcegroupstaggingapi|AddTags|NotImplemented
Issue-Key: autoscaling__ec2__elbv2|AddTags|NotImplemented
Issue-Key: elbv2|CreateTrustStore|NotImplemented
Issue-Key: elbv2.create-trust-store-invalid-ca-bundle-rejected|run#7|AssertionError|assert code == "InvalidCaCertificatesBundle", f"CODE={code!r}"
Issue-Key: elbv2.trust-store-ca-bundle-server-side-validation-on-create|run#9|AssertionError|assert code == "InvalidCaCertificatesBundle", (
Issue-Key: elbv2.trust-store-invalid-bundle-rejected|run#14|AssertionError|assert code == "InvalidCaCertificatesBundle", (
Issue-Key: elbv2.trust-store-bucket-not-found-error|run#11|AssertionError|assert code == "CaCertificatesBundleNotFound", code
Issue-Key: elbv2.trust-store-not-found-family|run#6|AssertionError|assert code == "TrustStoreNotFound", f"expected TrustStoreNotFound modifying a fabricated trust store, got {code!r}"
Issue-Key: elbv2|DeleteTargetGroup|NotImplemented
Issue-Key: elbv2.e2e-target-group-deletion-while-referenced-rejected|run#19|AssertionError|assert code == "ResourceInUse", f"expected ResourceInUse while referenced, got {code!r}"
Issue-Key: cloudtrail__elbv2|DeleteTargetGroup|NotImplemented
Issue-Key: elbv2|CreateRule|NotImplemented
Issue-Key: elbv2.network-config-error-semantics|run#21|AssertionError|assert err == ("LoadBalancerNotFound", f"Load balancer '{missing}' not found"), err
Issue-Key: elbv2.set-subnets-mismatched-vpc-rejected|run#14|AssertionError|assert code == "InvalidConfigurationRequest", f"expected InvalidConfigurationRequest, got {code}"
Issue-Key: elbv2.capacity-reservation-exceeds-limit-rejected|run#9|AssertionError|assert huge_code == "OperationNotPermitted", f"code={huge_code!r}"
Issue-Key: elbv2.server-side-capacity-reservation-minimum-units-floor-enforced|run#7|AssertionError|assert code is None, f"unexpected error requesting capacity reservation: {code}"
Issue-Key: elbv2|DescribeAccountLimits|NotImplemented
Issue-Key: elbv2.describe-ssl-policies-unknown-name-rejected|run#4|AssertionError|assert code == "SSLPolicyNotFound", (
Issue-Key: elbv2.describe-ssl-policies-malformed-name-filter-rejected|run#3|AssertionError|assert code == "SSLPolicyNotFound", f"expected SSLPolicyNotFound, got {code!r}"
Issue-Key: elbv2.describe-ssl-policies-malformed-name-format-rejected|run#7|AssertionError|assert not_found_code == "SSLPolicyNotFound", f"CODE={not_found_code!r}"
Issue-Key: elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected|run#7|AssertionError|assert code == "ValidationError", f"an HTTP probe modified in with no Matcher was answered {code!r}"
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# ELBv2: ten control-plane operation families answer 501 NotImplemented (tagging, trust stores, DeleteTargetGroup, CreateRule, SetSubnets/SetIpAddressType, capacity reservation, account limits)

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTrustStore

## Description
TEN distinct root causes are bundled here; all ten must be closed before this issue is done. Each is a separate modeled ELBv2 operation whose handler is a scaffold stub answering HTTP 501 `NotImplemented` instead of the modeled response or the modeled error. The pattern is identical, the operations are not.

Cause 1 of 10 — AddTags / RemoveTags (resource tagging).
AWS: Smithy AddTags(ResourceArns: required list, Tags: required TagList @length min 1) -> AddTagsOutput 200; errors DuplicateTagKeysException (awsQueryError DuplicateTagKeys, 400), TargetGroupNotFoundException (TargetGroupNotFound, 400), LoadBalancerNotFoundException (LoadBalancerNotFound, 400), ListenerNotFound, RuleNotFound, TrustStoreNotFound, TooManyTags. Tag keys/values round-trip byte-for-byte through AddTags/DescribeTags, an empty value is distinguished from an absent one, and two entries with the same Key in one request are refused with 400 DuplicateTagKeys.
Emulator: every AddTags/RemoveTags call -> 501 NotImplemented ("operation AddTags is scaffolded but not implemented in this emulator"). The duplicate-key checks see code 'NotImplemented' where DuplicateTagKeys is required.
Why: tags are unusable on every ELBv2 resource — no cost allocation, no tag-based discovery (resource-groups-tagging-api GetResources returns nothing for ELBv2), and negative tag validation silently disappears.
Fix: the AddTags/RemoveTags handlers and the tag store they write in the elasticloadbalancingv2 service crate.
Traces: elbv2.add-tags-duplicate-keys-rejected, elbv2.add-tags-nonexistent-resource-fails, elbv2.duplicate-tag-key-in-single-request-rejected, elbv2.create-load-balancer-empty-vs-absent-tags-preserved, elbv2.empty-string-vs-absent-tag-value-distinguished, elbv2.remove-tags-then-verify-and-reject-missing-resource, elbv2.tag-value-unicode-and-special-characters-round-trip, elbv2.tag-value-unicode-round-trip, elbv2.target-group-tag-unicode-value-round-trip, elbv2.unicode-tag-value-round-trip, elbv2.modify-capacity-reservation-invalid-unit-count-rejected, elbv2.modify-capacity-reservation-units-out-of-range-rejected, elbv2__resource-groups-tagging-api.load-balancer-and-target-group-tags-visible-through-get-resources, autoscaling__ec2__elbv2.elb-target-health-change-event-not-delivered-without-eventbridge-rule.

Cause 2 of 10 — CreateTrustStore and the whole mTLS trust-store family.
AWS: Smithy CreateTrustStore(Name required @length 1..32 @pattern ^([a-zA-Z0-9]+-)*[a-zA-Z0-9]+$, CaCertificatesBundleS3Bucket required, CaCertificatesBundleS3Key required, optional S3ObjectVersion, Tags) -> TrustStores 200; errors CaCertificatesBundleNotFoundException (CaCertificatesBundleNotFound 400), InvalidCaCertificatesBundleException (InvalidCaCertificatesBundle 400), DuplicateTrustStoreNameException (DuplicateTrustStoreName 400), DuplicateTagKeys, TooManyTrustStores, TooManyTags. The bundle is validated synchronously; the store transitions to ACTIVE; GetTrustStoreCaCertificatesBundle / GetTrustStoreRevocationContent return the uploaded bytes unchanged; ModifyTrustStore, AddTrustStoreRevocations, DescribeTrustStoreAssociations, DeleteTrustStore (ResourceInUse while associated) and GetResourcePolicy complete the surface.
Emulator: CreateTrustStore -> 501 NotImplemented, so the whole family is unreachable; the rejection checks observe code 'NotImplemented' where InvalidCaCertificatesBundle / TrustStoreNotFound / CaCertificatesBundleNotFound are required.
Why: mutual TLS cannot be configured at all, and every trust-store negative class (malformed PEM, missing bucket, unknown revocation id, in-use delete) is answered with a non-modeled code a client cannot branch on.
Fix: the CreateTrustStore handler plus the trust-store/revocation state and the Get*/Modify*/Delete* handlers in the elasticloadbalancingv2 crate.
Traces: elbv2.create-trust-store-reaches-active, elbv2.create-trust-store-duplicate-name-fails, elbv2.create-trust-store-invalid-ca-bundle-rejected, elbv2.trust-store-ca-bundle-server-side-validation-on-create, elbv2.trust-store-invalid-bundle-rejected, elbv2.trust-store-bucket-not-found-error, elbv2.trust-store-not-found-family, elbv2.trust-store-ops-not-found, elbv2.trust-store-lifecycle, elbv2.trust-store-lifecycle-create-active-delete-404, elbv2.trust-store-lifecycle-create-modify-delete, elbv2.trust-store-create-active-delete-404, elbv2.trust-store-creation-state-transition, elbv2.trust-store-name-dedup, elbv2.trust-store-modify-updates-bundle, elbv2.trust-store-in-use-delete-fails, elbv2.trust-store-association-lifecycle, elbv2.trust-store-association-and-shared-deletion-rejected, elbv2.trust-store-ca-bundle-retrieval, elbv2.trust-store-ca-bundle-binary-content-byte-equality, elbv2.trust-store-revocation-lifecycle, elbv2.trust-store-revocation-add-describe-remove, elbv2.trust-store-revocation-add-describe-remove-lifecycle, elbv2.trust-store-revocation-add-describe-remove-roundtrip, elbv2.add-trust-store-revocation-invalid-content-rejected, elbv2.add-trust-store-revocations-invalid-content-fails, elbv2.add-trust-store-revocations-malformed-crl-rejected, elbv2.add-trust-store-revocations-too-many-entries-fails, elbv2.remove-trust-store-revocations-unknown-id-not-silently-accepted, elbv2.delete-trust-store-in-use-rejected, elbv2.delete-trust-store-in-use-by-listener-fails, elbv2.delete-trust-store-then-not-found, elbv2.describe-trust-store-associations-for-shared-listener, elbv2.modify-trust-store-updates-ca-bundle, elbv2.modify-trust-store-invalid-bundle-fails, elbv2.modify-trust-store-missing-bundle-rejected, elbv2.modify-trust-store-nonexistent-bundle-rejected, elbv2.get-trust-store-ca-bundle-byte-equality, elbv2.get-trust-store-ca-bundle-byte-equality-round-trip, elbv2.get-trust-store-ca-bundle-byte-fidelity, elbv2.get-trust-store-ca-bundle-returns-full-uploaded-bundle, elbv2.get-trust-store-ca-bundle-unknown-truststore-fails, elbv2.get-trust-store-revocation-content-byte-equality, elbv2.get-trust-store-revocation-content-byte-equality-round-trip, elbv2.get-trust-store-revocation-content-byte-fidelity, elbv2.get-trust-store-revocation-content-matches-added, elbv2.get-trust-store-revocation-content-returns-full-crl-bytes, elbv2.get-trust-store-revocation-content-unknown-id-fails, elbv2.get-resource-policy-consistent-read, elbv2.get-resource-policy-no-policy-rejected, elbv2.get-resource-policy-nonexistent-fails, elbv2.get-resource-policy-returns-attached-json-policy, elbv2.get-resource-policy-returns-matching-policy-document, elbv2.resource-policy-not-found, elbv2.e2e-trust-store-mtls-association-and-revocation-flow.

Cause 3 of 10 — DeleteTargetGroup.
AWS: Smithy DeleteTargetGroup(TargetGroupArn required) -> 200 empty; its ONE modeled error is ResourceInUseException (awsQueryError ResourceInUse, 400), raised while the group is referenced by a listener or rule.
Emulator: 501 NotImplemented, so a referenced group's deletion is neither refused with ResourceInUse nor performed, and CloudTrail cannot audit the refusal with its exception-suffixed code.
Why: target groups can never be deleted, and the in-use guard that protects a live listener is unobservable.
Fix: the DeleteTargetGroup handler and its in-use reference check in the elasticloadbalancingv2 crate.
Traces: elbv2.delete-target-group-in-use-fails, elbv2.e2e-target-group-deletion-while-referenced-rejected, cloudtrail__elbv2.obs-elbv2-3-refused-target-group-calls-are-audited-with-exception-suffixed-codes.

Cause 4 of 10 — CreateRule (and the rule surface it gates).
AWS: Smithy CreateRule(ListenerArn, Conditions, Priority @range 1..50000, Actions all required) -> Rules 200; errors include PriorityInUseException (PriorityInUse 400), InvalidConfigurationRequest, InvalidLoadBalancerAction, TargetGroupNotFound, TooManyRules.
Emulator: 501 NotImplemented, so ModifyRule/SetRulePriorities and listener default-action representation can never be exercised.
Why: path/host routing — the core ALB feature — cannot be configured.
Fix: the CreateRule handler plus rule state (priority uniqueness per listener) in the elasticloadbalancingv2 crate.
Traces: elbv2.listener-default-action-empty-vs-absent-fields-preserved, elbv2.modify-rule-updates-actions-and-conditions, elbv2.set-rule-priorities-ignores-other-listener.

Cause 5 of 10 — SetIpAddressType.
AWS: Smithy SetIpAddressType(LoadBalancerArn, IpAddressType both required) -> {IpAddressType} 200; errors InvalidConfigurationRequestException (InvalidConfigurationRequest 400), InvalidSubnetException (InvalidSubnet 400), LoadBalancerNotFoundException. Switching to dualstack without IPv6 subnet CIDRs is refused with InvalidConfigurationRequest.
Emulator: 501 NotImplemented ('NotImplemented', 'operation SetIpAddressType is scaffolded but not implemented in this emulator').
Why: the network-configuration error family is indistinguishable from an unimplemented service.
Fix: the SetIpAddressType handler and its subnet/IPv6 validation.
Traces: elbv2.network-config-error-semantics.

Cause 6 of 10 — SetSubnets.
AWS: Smithy SetSubnets(LoadBalancerArn required, Subnets/SubnetMappings) -> AvailabilityZones 200; errors InvalidConfigurationRequestException (InvalidConfigurationRequest 400), InvalidSubnetException, SubnetNotFoundException, AvailabilityZoneNotSupportedException. Subnets from a different VPC than the load balancer are refused with InvalidConfigurationRequest.
Emulator: 501 NotImplemented — the check records "expected InvalidConfigurationRequest, got NotImplemented".
Why: a cross-VPC subnet swap is neither applied nor refused with the code clients retry/branch on.
Fix: the SetSubnets handler and its VPC/AZ validation.
Traces: elbv2.set-subnets-mismatched-vpc-rejected.

Cause 7 of 10 — ModifyCapacityReservation.
AWS: Smithy ModifyCapacityReservation(LoadBalancerArn required, MinimumLoadBalancerCapacity, ResetCapacityReservation) -> {LastModifiedTime, DecreaseRequestsRemaining, MinimumLoadBalancerCapacity, CapacityReservationState} 200; errors CapacityUnitsLimitExceededException (CapacityUnitsLimitExceeded 400), CapacityDecreaseRequestsLimitExceededException, CapacityReservationPendingException, InvalidConfigurationRequestException, OperationNotPermittedException, PriorRequestNotCompleteException (429), InsufficientCapacityException (500). An above-quota CapacityUnits request is refused with CapacityUnitsLimitExceeded; a below-floor one with InvalidConfigurationRequest.
Emulator: 501 NotImplemented (checks record code='NotImplemented' / "unexpected error requesting capacity reservation: NotImplemented").
Why: LCU pre-warming cannot be requested and its quota/floor rejections are unobservable.
Fix: the ModifyCapacityReservation handler and the per-LB capacity state.
Traces: elbv2.capacity-reservation-exceeds-limit-rejected, elbv2.server-side-capacity-reservation-minimum-units-floor-enforced.

Cause 8 of 10 — DescribeAccountLimits.
AWS: Smithy DescribeAccountLimits(Marker, PageSize @range 1..400) -> {Limits, NextMarker} 200 with NO modeled errors — it always succeeds, listing the account's ELBv2 limits (Name/Max pairs) and is @paginated on Marker/NextMarker/Limits.
Emulator: 501 NotImplemented.
Why: an operation AWS models as infallible fails; quota-aware clients and consoles break on their first call.
Fix: the DescribeAccountLimits handler (static limit catalogue + Marker pagination).
Traces: elbv2.op-contract-describe-account-limits.

Cause 9 of 10 — DescribeSSLPolicies rejection paths.
AWS: DescribeSSLPolicies with an unknown or malformed policy Name is refused with SSLPolicyNotFound (400); a well-formed request returns the policy catalogue in a stable order.
Emulator: 501 NotImplemented, so the checks see 'NotImplemented' where SSLPolicyNotFound is required.
Why: a client cannot tell an unknown SSL policy name from an unavailable service.
Fix: the DescribeSSLPolicies handler, including its name-filter validation. (The bare stub is covered by the suite's auto-filed DescribeSSLPolicies stub issue, which does not carry these three rejection replay keys.)
Traces: elbv2.describe-ssl-policies-unknown-name-rejected, elbv2.describe-ssl-policies-malformed-name-filter-rejected, elbv2.describe-ssl-policies-malformed-name-format-rejected.

Cause 10 of 10 — ModifyTargetGroup health-check validation path.
AWS: ModifyTargetGroup switching a TCP health probe to HTTP without a Matcher is refused with ValidationError/InvalidConfigurationRequest (400); HealthCheckThresholdCount is @range 2..10 and HealthCheckIntervalSeconds @range 5..300 per the model.
Emulator: 501 NotImplemented — "an HTTP probe modified in with no Matcher was answered 'NotImplemented'".
Why: health-check reconfiguration is impossible and its validation boundary is invisible.
Fix: the ModifyTargetGroup handler and its health-check validation. (The bare stub is covered by the suite's auto-filed ModifyTargetGroup stub issue, which does not carry this replay key.)
Traces: elbv2.asgdx-modify-tcp-probe-to-http-without-matcher-rejected.

Related open issues that record the OLDER symptom (400 InvalidAction) rather than today's 501 and so do not cover these keys: 20261023T060746Z-elasticloadbalancing-elbv2-trust-store-create-modify-delete-and.md, 20261023T060746Z-elasticloadbalancing-elbv2-trust-store-revocation-ca-bundle-target.md, 20261023T060746Z-elasticloadbalancing-elbv2-load-balancer-attribute-network-capacity-and.md, 20261023T060746Z-elasticloadbalancing-elbv2-listener-certificate-and-rule-actions-answer.md, 20261023T060746Z-elasticloadbalancing-elbv2-target-group-and-target-registration-actions.md.

## Evidence
Expected (per operation): the modeled 200 response, or the modeled 400 error code — DuplicateTagKeys (AddTags), InvalidCaCertificatesBundle / DuplicateTrustStoreName / CaCertificatesBundleNotFound / TrustStoreNotFound (trust stores), ResourceInUse (DeleteTargetGroup), PriorityInUse (CreateRule), InvalidConfigurationRequest / InvalidSubnet (SetIpAddressType, SetSubnets), CapacityUnitsLimitExceeded (ModifyCapacityReservation), SSLPolicyNotFound (DescribeSSLPolicies), a 200 Limits list (DescribeAccountLimits).
Actual: HTTP 501 with Error.Code=NotImplemented for all of them; message (example) "operation <Op> is scaffolded but not implemented in this emulator". Replay excerpts: "expected DuplicateTagKeys, got 'NotImplemented'"; "expected InvalidCaCertificatesBundle for a malformed CA bundle, got 'NotImplemented'"; "expected error code in ('ResourceInUse',), got 'NotImplemented'"; "expected InvalidConfigurationRequest, got NotImplemented"; "expected SSLPolicyNotFound for an unknown policy name, got 'NotImplemented'"; "unexpected error requesting capacity reservation: NotImplemented".

## Steps to Reproduce
1. Create a VPC with two subnets, then an application load balancer and a target group (both already work).
2. Cause 1: AddTags(ResourceArns=[tg_arn], Tags=[{Key:'k',Value:'v'},{Key:'k',Value:'v2'}]) -> expect 400 DuplicateTagKeys, observe 501 NotImplemented.
3. Cause 2: upload a PEM CA bundle to S3, CreateTrustStore(Name=..., CaCertificatesBundleS3Bucket=..., CaCertificatesBundleS3Key=...) -> expect 200 TrustStores, observe 501.
4. Cause 3: attach the target group to a listener, DeleteTargetGroup(tg_arn) -> expect 400 ResourceInUse, observe 501.
5. Cause 4: CreateRule(ListenerArn, Conditions=[path /a], Priority=1, Actions=[forward]) -> expect 200 Rules, observe 501.
6. Causes 5-8: SetIpAddressType, SetSubnets (subnets from another VPC), ModifyCapacityReservation(MinimumLoadBalancerCapacity above quota), DescribeAccountLimits() -> all observe 501.
7. Cause 9: DescribeSSLPolicies(Names=['no-such-policy']) -> expect 400 SSLPolicyNotFound, observe 501.
8. Cause 10: ModifyTargetGroup on a TCP group with HealthCheckProtocol=HTTP and no Matcher -> expect 400 validation error, observe 501.
