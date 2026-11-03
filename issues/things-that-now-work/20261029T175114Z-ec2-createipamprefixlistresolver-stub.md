Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-delete-ipam-prefix-list-resolver ec2.op-contract-delete-ipam-prefix-list-resolver-target
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateIpamPrefixListResolver|NotImplemented
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# Implement ec2 CreateIpamPrefixListResolver: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIpamPrefixListResolver

## Description
`CreateIpamPrefixListResolver` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam_prefix_list_resolver.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CreateIpamPrefixListResolver` call, so each one unblocks only when `CreateIpamPrefixListResolver` works: `ec2.op-contract-delete-ipam-prefix-list-resolver`, `ec2.op-contract-delete-ipam-prefix-list-resolver-target`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_ipam_prefix_list_resolver.rs`. Replaying the corpus, the emulator refused the `CreateIpamPrefixListResolver` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_ec2_op_contract_delete_ipam_prefix_list_resolver.py", line 59, in run
    raise AssertionError(
AssertionError: unreachable: CreateIpamPrefixListResolver rejected in this account/region: NotImplemented

## Steps to Reproduce
Boot the ec2 emulator
Call CreateIpamPrefixListResolver and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
