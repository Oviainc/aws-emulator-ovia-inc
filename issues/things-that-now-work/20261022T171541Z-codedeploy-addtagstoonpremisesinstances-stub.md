Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.tag-on-premises-instance-not-registered-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.tag-on-premises-instance-not-registered-rejected|run#6|AssertionError|assert error == (
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Implement codedeploy AddTagsToOnPremisesInstances: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddTagsToOnPremisesInstances

## Description
`AddTagsToOnPremisesInstances` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/add_tags_to_on_premises_instances.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `AddTagsToOnPremisesInstances` call, so each one unblocks only when `AddTagsToOnPremisesInstances` works: `codedeploy.tag-on-premises-instance-not-registered-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/add_tags_to_on_premises_instances.rs`. Replaying the corpus, the emulator refused the `AddTagsToOnPremisesInstances` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_codedeploy_tag_on_premises_instance_not_registered_rejected.py", line 27, in run
    assert error == (
           ^^^^^^^^^^
    ...<2 lines>...
    ), f"expected InstanceNotRegisteredException tagging an unregistered instance, got {error!r}"
    ^
AssertionError: expected InstanceNotRegisteredException tagging an unregistered instance, got ('NotImplemented', 'operation AddTagsToOnPremisesInstances is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the codedeploy emulator
Call AddTagsToOnPremisesInstances and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
