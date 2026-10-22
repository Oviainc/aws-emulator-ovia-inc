Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.flow-log-fleet-and-gateway-error-semantics ec2.flow-logs-integration-template-parameterized-by-config ec2.op-contract-get-flow-logs-integration-template ec2__iam__logs.vpc-flow-log-to-an-absent-log-group-is-accepted-without-creating-it ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-without-a-delivery-role-rejected ec2__logs__s3.s3-and-cloudwatch-flow-logs-on-one-vpc-partition-by-destination-type ec2__s3.deleting-an-already-deleted-flow-log-rejected ec2__s3.vpc-flow-log-hive-prefix-grant-covers-the-partitioned-path ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy
Issue-Key: ec2.flow-log-fleet-and-gateway-error-semantics|run#4|AssertionError|assert not problems, "unexpected flow-log/gateway error semantics:\n" + "\n".join(problems)
Issue-Key: ec2|CreateFlowLogs|NotImplemented
Issue-Key: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-without-a-delivery-role-rejected|run#4|AssertionError|assert code == "InvalidParameter", (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 CreateFlowLogs: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
`CreateFlowLogs` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

10 recorded unit(s) fail on their `CreateFlowLogs` call, so each one unblocks only when `CreateFlowLogs` works: `ec2.flow-log-fleet-and-gateway-error-semantics`, `ec2.flow-logs-integration-template-parameterized-by-config`, `ec2.op-contract-get-flow-logs-integration-template`, `ec2__iam__logs.vpc-flow-log-to-an-absent-log-group-is-accepted-without-creating-it`, `ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields`, `ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-without-a-delivery-role-rejected`, `ec2__logs__s3.s3-and-cloudwatch-flow-logs-on-one-vpc-partition-by-destination-type`, `ec2__s3.deleting-an-already-deleted-flow-log-rejected`, `ec2__s3.vpc-flow-log-hive-prefix-grant-covers-the-partitioned-path`, `ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs`. Replaying the corpus, the emulator refused the `CreateFlowLogs` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_ec2_flow_log_fleet_and_gateway_error_semantics.py", line 83, in run
    assert not problems, "unexpected flow-log/gateway error semantics:\n" + "\n".join(problems)
           ^^^^^^^^^^^^
AssertionError: unexpected flow-log/gateway error semantics:
create_flow_logs: expected ('InvalidVpcId.NotFound', 'Unknown resource vpc-0123456789abcdef0'), got ('NotImplemented', 'operation CreateFlowLogs is scaffolded but not implemented in this emulator')
delete_flow_logs: expected ('InvalidFlowLogId.NotFound', 'These flow log ids in the input list are not found: [TotalCount: 1] fl-0123456789abcdef0'), got ('NotImplemented', 'operation DeleteFlowLogs is scaffolded but not implemented in this emulator')
create_carrier_gateway: expected ('InvalidVpcId.Malformed', "The vpc ID 'vpc-0123456789abcdef0' is malformed"), got ('NotImplemented', 'operation CreateCarrierGateway is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call CreateFlowLogs and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
