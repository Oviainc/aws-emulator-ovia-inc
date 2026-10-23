Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: notificationscontacts.activate-without-pending-code-rejected notificationscontacts.concurrent-tagging-distinct-keys-preserved notificationscontacts.email-contact-identity-stable-after-tag-operations notificationscontacts.tag-resource-then-list-tags-roundtrip notificationscontacts.untag-resource-nonexistent-key-noop notificationscontacts.untag-resource-removes-specific-tag
Issue-Shape-Id: com.amazonaws.notificationscontacts#NotificationsContacts
Issue-Key: notificationscontacts|TagResource|NotImplemented
Issue-Commit: 3e963f69e1d0897f40654d15e23b2fa6367d6b7c

# Implement notificationscontacts TagResource: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `3e963f69e1d0897f40654d15e23b2fa6367d6b7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
`TagResource` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/tag_resource.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `TagResource` call, so each one unblocks only when `TagResource` works: `notificationscontacts.activate-without-pending-code-rejected`, `notificationscontacts.concurrent-tagging-distinct-keys-preserved`, `notificationscontacts.email-contact-identity-stable-after-tag-operations`, `notificationscontacts.tag-resource-then-list-tags-roundtrip`, `notificationscontacts.untag-resource-nonexistent-key-noop`, `notificationscontacts.untag-resource-removes-specific-tag`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-notificationscontacts-notificationscontacts/src/ops/tag_resource.rs`. Replaying the corpus, the emulator refused the `TagResource` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagResource operation: operation TagResource is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the notificationscontacts emulator
Call TagResource and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
