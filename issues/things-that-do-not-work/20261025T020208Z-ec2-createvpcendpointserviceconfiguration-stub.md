Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.managed-insight-rule-report-reflects-applied-template cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item cloudwatch.put-managed-insight-rules-invalid-template-rejected
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: ec2|CreateVpcEndpointServiceConfiguration|NotImplemented
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Implement ec2 CreateVpcEndpointServiceConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpointServiceConfiguration

## Description
`CreateVpcEndpointServiceConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint_service_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `CreateVpcEndpointServiceConfiguration` call, so each one unblocks only when `CreateVpcEndpointServiceConfiguration` works: `cloudwatch.managed-insight-rule-report-reflects-applied-template`, `cloudwatch.put-managed-insight-rules-invalid-template-name-rejected-per-item`, `cloudwatch.put-managed-insight-rules-invalid-template-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_vpc_endpoint_service_configuration.rs`. Replaying the corpus, the emulator refused the `CreateVpcEndpointServiceConfiguration` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_cloudwatch_managed_insight_rule_report_reflects_applied_template.py", line 49, in run
    raise AssertionError(f"could not provision a VPC endpoint service prerequisite: {exc}") from exc
AssertionError: could not provision a VPC endpoint service prerequisite: An error occurred (NotImplemented) when calling the CreateVpcEndpointServiceConfiguration operation: operation CreateVpcEndpointServiceConfiguration is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateVpcEndpointServiceConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
