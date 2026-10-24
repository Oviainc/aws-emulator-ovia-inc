Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.check-modify-volume-size-decrease-rejected ec2.modify-volume-iops-below-minimum-accepted ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared ec2.op-contract-describe-volumes-modifications ec2.op-contract-modify-volume
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ModifyVolume|NotImplemented
Issue-Key: ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared|run#10|AssertionError|assert code == ERR_BAD_COMBINATION, (
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Implement ec2 ModifyVolume: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyVolume

## Description
`ModifyVolume` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_volume.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `ModifyVolume` call, so each one unblocks only when `ModifyVolume` works: `ec2.check-modify-volume-size-decrease-rejected`, `ec2.modify-volume-iops-below-minimum-accepted`, `ec2.multiattach-modify-of-an-io2-volume-to-gp3-is-refused-until-the-flag-is-cleared`, `ec2.op-contract-describe-volumes-modifications`, `ec2.op-contract-modify-volume`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_volume.rs`. Replaying the corpus, the emulator refused the `ModifyVolume` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyVolume operation: operation ModifyVolume is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call ModifyVolume and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
