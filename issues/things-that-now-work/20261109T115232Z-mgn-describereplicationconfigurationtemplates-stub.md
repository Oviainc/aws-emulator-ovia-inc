Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.replication-configuration-template-access-denied-rejected
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn|DescribeReplicationConfigurationTemplates|NotImplemented
Issue-Commit: c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3

# Implement mgn DescribeReplicationConfigurationTemplates: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c0b75f3dd0ad77b20e3163eb81f4603ab19da1b3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReplicationConfigurationTemplates

## Description
`DescribeReplicationConfigurationTemplates` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/describe_replication_configuration_templates.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DescribeReplicationConfigurationTemplates` call, so each one unblocks only when `DescribeReplicationConfigurationTemplates` works: `mgn.replication-configuration-template-access-denied-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/describe_replication_configuration_templates.rs`. Replaying the corpus, the emulator refused the `DescribeReplicationConfigurationTemplates` call: File "check_mgn_replication_configuration_template_access_denied_rejected.py", line 24, in run
    _ensure_initialized(mgn)
  File "check_mgn_replication_configuration_template_access_denied_rejected.py", line 195, in _ensure_initialized
    raise AssertionError(
AssertionError: account did not become initialized; InitializeService itself answered None, last describe answered 'NotImplemented'

## Steps to Reproduce
Boot the mgn emulator
Call DescribeReplicationConfigurationTemplates and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
