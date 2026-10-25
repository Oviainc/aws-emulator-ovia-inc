Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates ec2.enabled-ipam-policy-reflects-allocation-rule-changes ec2.ipam-default-scope-deletion-rejected ec2.ipam-delete-with-dependents-rejected-until-removed-in-order ec2.ipam-deprovision-cidr-under-live-vpc-accepted-then-rejected-as-failed-deprovision ec2.ipam-discovered-accounts-max-results-below-minimum-rejected ec2.ipam-discovered-public-addresses-reflects-scan-results ec2.ipam-discovered-resource-cidrs-reflects-scan-results ec2.ipam-internet-registry-association-asns-listing ec2.ipam-internet-registry-association-asns-matches-association ec2.ipam-policy-allocation-rule-private-pool-rejected ec2.ipam-pool-with-provisioned-cidr-deletion-rejected ec2.ipam-resource-cidr-wrong-current-scope-rejected ec2.ipam-routing-policy-registration-round-trip ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules ec2.ipam-vpc-from-regional-child-pool-found-by-recursive-allocation-walk ec2.ipam-vpc-on-custom-reserved-cidr-rejected ec2.modify-ipam-resource-cidr-updates-monitoring-state ec2.op-contract-allocateIpamPoolCidr ec2.op-contract-delete-ipam ec2.op-contract-delete-ipam-external-resource-verification-token ec2.op-contract-delete-ipam-policy ec2.op-contract-delete-ipam-pool ec2.op-contract-delete-ipam-prefix-list-resolver ec2.op-contract-delete-ipam-prefix-list-resolver-target ec2.op-contract-delete-ipam-scope ec2.op-contract-deprovision-ipam-pool-cidr ec2.op-contract-describe-ipam-resource-discoveries-superseded-stub ec2.op-contract-describe-ipam-resource-discovery-associations ec2.op-contract-describe-ipam-resource-discovery-associations-superseded-stub ec2.op-contract-describe-ipam-scopes ec2.op-contract-describe-ipam-scopes-superseded-stub ec2.op-contract-describe-ipams ec2.op-contract-describe-ipams-superseded-stub ec2.op-contract-describeIpamExternalResourceVerificationTokens ec2.op-contract-describeIpamInternetRegistryAssociations ec2.op-contract-describeIpamPolicies ec2.op-contract-describeIpamPoolAllocations ec2.op-contract-describeIpamPools ec2.op-contract-disable-ipam-policy ec2.op-contract-enableIpamPolicy ec2.op-contract-get-ipam-discovered-accounts ec2.op-contract-get-ipam-discovered-public-addresses ec2.op-contract-get-ipam-discovered-resource-cidrs ec2.op-contract-get-ipam-discovered-routes ec2.op-contract-get-ipam-internet-registry-association-asns ec2.op-contract-get-ipam-internet-registry-association-cidrs ec2.op-contract-get-ipam-policy-allocation-rules ec2.op-contract-get-ipam-policy-organization-targets ec2.op-contract-get-ipam-pool-allocations ec2.op-contract-get-ipam-pool-cidrs ec2.op-contract-get-ipam-resource-cidrs ec2.op-contract-get-ipam-route-protection-findings-invalid-ipam-rejected ec2.op-contract-modify-ipam ec2.op-contract-modify-ipam-policy-allocation-rules ec2.op-contract-modify-ipam-pool ec2.op-contract-modify-ipam-prefix-list-resolver ec2.op-contract-modify-ipam-prefix-list-resolver-target ec2.op-contract-modify-ipam-scope ec2.op-contract-provision-ipam-pool-cidr ec2.op-contract-provision-public-ipv4-pool-cidr-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateIpam|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateIpam: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIpam

## Description
`CreateIpam` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

61 recorded unit(s) fail on their `CreateIpam` call, so each one unblocks only when `CreateIpam` works: `ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates`, `ec2.enabled-ipam-policy-reflects-allocation-rule-changes`, `ec2.ipam-default-scope-deletion-rejected`, `ec2.ipam-delete-with-dependents-rejected-until-removed-in-order`, `ec2.ipam-deprovision-cidr-under-live-vpc-accepted-then-rejected-as-failed-deprovision`, `ec2.ipam-discovered-accounts-max-results-below-minimum-rejected`, `ec2.ipam-discovered-public-addresses-reflects-scan-results`, `ec2.ipam-discovered-resource-cidrs-reflects-scan-results`, `ec2.ipam-internet-registry-association-asns-listing`, `ec2.ipam-internet-registry-association-asns-matches-association`, `ec2.ipam-policy-allocation-rule-private-pool-rejected`, `ec2.ipam-pool-with-provisioned-cidr-deletion-rejected`, `ec2.ipam-resource-cidr-wrong-current-scope-rejected`, `ec2.ipam-routing-policy-registration-round-trip`, `ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules`, `ec2.ipam-vpc-from-regional-child-pool-found-by-recursive-allocation-walk`, `ec2.ipam-vpc-on-custom-reserved-cidr-rejected`, `ec2.modify-ipam-resource-cidr-updates-monitoring-state`, `ec2.op-contract-allocateIpamPoolCidr`, `ec2.op-contract-delete-ipam` (+41 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam.rs`. Replaying the corpus, the emulator refused the `CreateIpam` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateIpam operation: operation CreateIpam is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateIpam and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
