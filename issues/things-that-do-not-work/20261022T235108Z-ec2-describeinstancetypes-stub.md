Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.cpuopts-core-count-outside-valid-cores-rejected ec2.cpuopts-guest-topology-differs-from-the-type-default ec2.cpuopts-partial-cpu-options-rejected ec2.cpuopts-threads-per-core-one-observed-in-guest ec2.cpuopts-topology-survives-a-stop-start-cycle ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest ec2.diops-io2-provisioned-iops-outrun-gp3-baseline-measured-by-fio-in-one-guest ec2.ec2-instance-store-bytes-vanish-across-stop-start-while-ebs-bytes-persist ec2.ec2-instance-type-change-is-visible-to-the-guest-and-matches-the-catalog ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume ec2.hibernate-launch-on-a-type-without-hibernation-support-rejected ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected ec2.hibernate-stop-with-hibernate-on-an-instance-not-configured-for-it-rejected ec2.nvme-instance-store-guest-device-path-rejected-as-an-ebs-attachment-point ec2.nvme-instance-store-namespace-is-nvme-while-the-xen-ebs-root-stays-xvd ec2.nvme-instance-store-serial-to-device-mapping-and-data-survive-a-reboot ec2.op-contract-describeInstanceTypes ec2.persist-hibernation-on-unencrypted-root-rejected ec2.placement-cluster-group-members-read-one-shared-zone-and-their-group-in-guest ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests ec2.placement-partition-number-above-the-group-count-rejected ec2.placement-spread-group-rejects-more-instances-than-one-zone-can-place ec2.secip-assigning-more-private-ips-than-the-instance-type-permits-rejected ec2.secip-hot-attached-second-eni-appears-in-the-guest-as-a-new-interface-with-its-own-address
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeInstanceTypes|NotImplemented
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 DescribeInstanceTypes: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstanceTypes

## Description
`DescribeInstanceTypes` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_types.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

24 recorded unit(s) fail on their `DescribeInstanceTypes` call, so each one unblocks only when `DescribeInstanceTypes` works: `ec2.cpuopts-core-count-outside-valid-cores-rejected`, `ec2.cpuopts-guest-topology-differs-from-the-type-default`, `ec2.cpuopts-partial-cpu-options-rejected`, `ec2.cpuopts-threads-per-core-one-observed-in-guest`, `ec2.cpuopts-topology-survives-a-stop-start-cycle`, `ec2.diops-gp3-iops-to-size-ratio-above-the-limit-rejected-and-no-iops-reach-the-guest`, `ec2.diops-io2-provisioned-iops-outrun-gp3-baseline-measured-by-fio-in-one-guest`, `ec2.ec2-instance-store-bytes-vanish-across-stop-start-while-ebs-bytes-persist`, `ec2.ec2-instance-type-change-is-visible-to-the-guest-and-matches-the-catalog`, `ec2.hibernate-guest-process-and-its-in-memory-value-survive-the-resume`, `ec2.hibernate-launch-on-a-type-without-hibernation-support-rejected`, `ec2.hibernate-launch-with-root-volume-too-small-for-ram-rejected`, `ec2.hibernate-stop-with-hibernate-on-an-instance-not-configured-for-it-rejected`, `ec2.nvme-instance-store-guest-device-path-rejected-as-an-ebs-attachment-point`, `ec2.nvme-instance-store-namespace-is-nvme-while-the-xen-ebs-root-stays-xvd`, `ec2.nvme-instance-store-serial-to-device-mapping-and-data-survive-a-reboot`, `ec2.op-contract-describeInstanceTypes`, `ec2.persist-hibernation-on-unencrypted-root-rejected`, `ec2.placement-cluster-group-members-read-one-shared-zone-and-their-group-in-guest`, `ec2.placement-partition-group-serves-distinct-partition-numbers-to-the-guests` (+4 more).

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/describe_instance_types.rs`. Replaying the corpus, the emulator refused the `DescribeInstanceTypes` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeInstanceTypes operation: operation DescribeInstanceTypes is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call DescribeInstanceTypes and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
