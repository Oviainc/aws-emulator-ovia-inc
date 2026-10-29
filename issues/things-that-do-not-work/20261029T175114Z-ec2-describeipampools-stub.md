Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates ec2.enabled-ipam-policy-reflects-allocation-rule-changes ec2.ipam-delete-with-dependents-rejected-until-removed-in-order ec2.ipam-deprovision-cidr-under-live-vpc-accepted-then-rejected-as-failed-deprovision ec2.ipam-policy-allocation-rule-private-pool-rejected ec2.ipam-pool-with-provisioned-cidr-deletion-rejected ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules ec2.ipam-vpc-from-regional-child-pool-found-by-recursive-allocation-walk ec2.ipam-vpc-on-custom-reserved-cidr-rejected ec2.op-contract-allocateIpamPoolCidr ec2.op-contract-allocateIpamPoolCidr-b ec2.op-contract-delete-ipam-pool ec2.op-contract-describeIpamPoolAllocations ec2.op-contract-describeIpamPools ec2.op-contract-get-ipam-address-history ec2.op-contract-get-ipam-pool-allocations ec2.op-contract-get-ipam-pool-cidrs ec2.op-contract-modify-ipam-pool ec2.op-contract-modify-ipam-pool-allocation ec2.op-contract-provision-ipam-pool-cidr ec2.op-contract-provision-public-ipv4-pool-cidr-rejected ec2.op-contract-release-ipam-pool-allocation
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeIpamPools|NotImplemented
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# Implement ec2 DescribeIpamPools: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeIpamPools

## Description
`DescribeIpamPools` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_pools.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

22 recorded unit(s) fail on their `DescribeIpamPools` call, so each one unblocks only when `DescribeIpamPools` works: `ec2.concurrent-modify-ipam-resource-cidr-conflicting-updates`, `ec2.enabled-ipam-policy-reflects-allocation-rule-changes`, `ec2.ipam-delete-with-dependents-rejected-until-removed-in-order`, `ec2.ipam-deprovision-cidr-under-live-vpc-accepted-then-rejected-as-failed-deprovision`, `ec2.ipam-policy-allocation-rule-private-pool-rejected`, `ec2.ipam-pool-with-provisioned-cidr-deletion-rejected`, `ec2.ipam-top-pool-reports-none-locale-depth-one-and-omits-allocation-rules`, `ec2.ipam-vpc-from-regional-child-pool-found-by-recursive-allocation-walk`, `ec2.ipam-vpc-on-custom-reserved-cidr-rejected`, `ec2.op-contract-allocateIpamPoolCidr`, `ec2.op-contract-allocateIpamPoolCidr-b`, `ec2.op-contract-delete-ipam-pool`, `ec2.op-contract-describeIpamPoolAllocations`, `ec2.op-contract-describeIpamPools`, `ec2.op-contract-get-ipam-address-history`, `ec2.op-contract-get-ipam-pool-allocations`, `ec2.op-contract-get-ipam-pool-cidrs`, `ec2.op-contract-modify-ipam-pool`, `ec2.op-contract-modify-ipam-pool-allocation`, `ec2.op-contract-provision-ipam-pool-cidr` (+2 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_ipam_pools.rs`. Replaying the corpus, the emulator refused the `DescribeIpamPools` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeIpamPools operation: operation DescribeIpamPools is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeIpamPools and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
