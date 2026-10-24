Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.cluster-security-group-ops-semantics
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.cluster-security-group-ops-semantics|run#5|AssertionError|assert create_code == "InvalidParameterValue", (
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Implement redshift CreateClusterSecurityGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateClusterSecurityGroup

## Description
`CreateClusterSecurityGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster_security_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateClusterSecurityGroup` call, so each one unblocks only when `CreateClusterSecurityGroup` works: `redshift.cluster-security-group-ops-semantics`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_cluster_security_group.rs`. Replaying the corpus, the emulator refused the `CreateClusterSecurityGroup` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_redshift_cluster_security_group_ops_semantics.py", line 90, in run
    assert create_code == "InvalidParameterValue", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: CreateClusterSecurityGroup is discontinued and must be rejected, got NotImplemented: 'operation CreateClusterSecurityGroup is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the redshift emulator
Call CreateClusterSecurityGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
