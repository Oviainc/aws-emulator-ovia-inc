Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.op-contract-generateOrganizationsAccessReport
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.op-contract-generateOrganizationsAccessReport|run#8|AssertionError|assert code == "AccessDenied", f"expected AccessDenied for a standalone caller, got {code!r}: {resp or message!r}"
Issue-Commit: 2e55e3f174321d59189949bdc160ce3c5e1a8b8d

# Implement iam GenerateOrganizationsAccessReport: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2e55e3f174321d59189949bdc160ce3c5e1a8b8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateOrganizationsAccessReport

## Description
`GenerateOrganizationsAccessReport` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/generate_organizations_access_report.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GenerateOrganizationsAccessReport` call, so each one unblocks only when `GenerateOrganizationsAccessReport` works: `iam.op-contract-generateOrganizationsAccessReport`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/ops/generate_organizations_access_report.rs`. Replaying the corpus, the emulator refused the `GenerateOrganizationsAccessReport` call: File "run.py", line 176, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_iam_op_contract_generateOrganizationsAccessReport.py", line 38, in run
    assert code == "AccessDenied", f"expected AccessDenied for a standalone caller, got {code!r}: {resp or message!r}"
           ^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected AccessDenied for a standalone caller, got 'NotImplemented': 'operation GenerateOrganizationsAccessReport is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the iam emulator
Call GenerateOrganizationsAccessReport and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
