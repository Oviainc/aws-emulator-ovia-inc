Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: license-manager-linux-subscriptions.tag-resource-list-untag-round-trip
Issue-Shape-Id: com.amazonaws.licensemanagerlinuxsubscriptions#LicenseManagerLinuxSubscriptions
Issue-Key: license-manager-linux-subscriptions.tag-resource-list-untag-round-trip|run#6|AssertionError|assert tag == _NOT_FOUND, (
Issue-Commit: 7545d0b8b2e4fb4a41c801fcbb48b523234ffec8

# Implement license-manager-linux-subscriptions TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7545d0b8b2e4fb4a41c801fcbb48b523234ffec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `license-manager-linux-subscriptions.tag-resource-list-untag-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-licensemanagerlinuxsubscriptions-licensemanagerlinuxsubscriptions/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_license_manager_linux_subscriptions_tag_resource_list_untag_round_trip.py", line 29, in run
    assert tag == _NOT_FOUND, (
           ^^^^^^^^^^^^^^^^^
AssertionError: TagResource on a never-registered subscription-provider ARN: expected ('NotFoundException', 'Resource was not found'), got ('NotImplemented', 'operation TagResource is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the license-manager-linux-subscriptions emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
