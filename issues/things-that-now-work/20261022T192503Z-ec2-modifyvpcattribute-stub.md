Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.cancel-job-run-pending-to-cancelled emr-containers.cancel-job-run-terminal-state-rejected emr-containers.create-managed-endpoint-invalid-type-rejected emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected emr-containers.delete-virtual-cluster-with-active-job-run-rejected emr-containers.list-job-runs-state-filter emr-containers.start-job-run-spark-pi-native-round-trip emr-containers.update-virtual-cluster-on-terminated-cluster-rejected emr-containers.virtual-cluster-delete-lifecycle-to-terminated emr-containers.virtual-cluster-tag-untag-round-trip emr-containers.virtual-cluster-update-scheduler-configuration-success
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: ec2|ModifyVpcAttribute|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ec2 ModifyVpcAttribute: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVpcAttribute

## Description
`ModifyVpcAttribute` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_attribute.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

11 recorded unit(s) fail on their `ModifyVpcAttribute` call, so each one unblocks only when `ModifyVpcAttribute` works: `emr-containers.cancel-job-run-pending-to-cancelled`, `emr-containers.cancel-job-run-terminal-state-rejected`, `emr-containers.create-managed-endpoint-invalid-type-rejected`, `emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected`, `emr-containers.delete-virtual-cluster-with-active-job-run-rejected`, `emr-containers.list-job-runs-state-filter`, `emr-containers.start-job-run-spark-pi-native-round-trip`, `emr-containers.update-virtual-cluster-on-terminated-cluster-rejected`, `emr-containers.virtual-cluster-delete-lifecycle-to-terminated`, `emr-containers.virtual-cluster-tag-untag-round-trip`, `emr-containers.virtual-cluster-update-scheduler-configuration-success`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_attribute.rs`. Replaying the corpus, the emulator refused the `ModifyVpcAttribute` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyVpcAttribute operation: operation ModifyVpcAttribute is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call ModifyVpcAttribute and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
