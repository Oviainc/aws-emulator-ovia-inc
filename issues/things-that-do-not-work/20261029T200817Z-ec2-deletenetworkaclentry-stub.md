Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-dry-run-delete-validation-across-network-resources
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-dry-run-delete-validation-across-network-resources ec2-network-acl-create-configure-entries-delete ec2-network-acl-entries-lifecycle ec2-network-acl-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteNetworkAclEntry
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DeleteNetworkAclEntry: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteNetworkAclEntry

## Description
`DeleteNetworkAclEntry` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_network_acl_entry.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

8 recorded unit(s) fail on their `DeleteNetworkAclEntry` call, so each one unblocks only when `DeleteNetworkAclEntry` works: `ec2-dry-run-delete-validation-across-network-resources#task#11`, `ec2-dry-run-delete-validation-across-network-resources#task#19`, `ec2-network-acl-create-configure-entries-delete#task#8`, `ec2-network-acl-create-configure-entries-delete#task#9`, `ec2-network-acl-create-configure-entries-delete#task#10`, `ec2-network-acl-entries-lifecycle#task#6`, `ec2-network-acl-lifecycle#teardown#7`, `ec2-network-acl-lifecycle#teardown#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_network_acl_entry.rs`. Replaying the corpus, the emulator refused the `DeleteNetworkAclEntry` call: status: expected 412, got 501
$.Response.Errors.Error.Code: value_mismatch (expected 'DryRunOperation', got 'NotImplemented')
$.Response.Errors.Error.Message: value_mismatch (expected 'Request would have succeeded, but DryRun flag is set.', got 'operation DeleteNetworkAclEntry is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteNetworkAclEntry and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
