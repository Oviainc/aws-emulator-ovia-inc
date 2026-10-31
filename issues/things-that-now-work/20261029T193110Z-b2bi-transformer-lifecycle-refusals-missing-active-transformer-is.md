Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.update-transformer-status-toggle-active-inactive b2bi.update-transformer-immutable-fields-once-active-rejected b2bi.delete-dependency-order-enforced-rejected
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi.update-transformer-status-toggle-active-inactive|_error#13|AssertionError|raise AssertionError(
Issue-Key: b2bi.update-transformer-immutable-fields-once-active-rejected|run#13|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: b2bi.delete-dependency-order-enforced-rejected|run#22|AssertionError|assert code == "ConflictException", f"expected ConflictException deleting a referenced transformer, got {code}"
Issue-Commit: 24dcbcf55b45bb9d0573a38e9c06262a825c087d

# b2bi transformer lifecycle refusals missing: active transformer is mutable, and DeleteTransformer answers 501 instead of enforcing dependencies

> _Note: this issue was identified against the code base at commit `24dcbcf55b45bb9d0573a38e9c06262a825c087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateTransformer

## Description
THREE net-new causes in the transformer lifecycle: two mutations AWS refuses are accepted or refused with the wrong error, and the delete path that AWS uses to enforce dependency order is not implemented at all.

Cause 1 of 3: UpdateTransformer accepts an active -> inactive status flip that AWS refuses.
AWS: Smithy UpdateTransformer models ValidationException (HTTP 400, member `Message`) among its errors; TransformerStatus's only documented transition is inactive -> active, and once a transformer is active it is immutable. The recorded refusal is ValidationException with message "An active Transformer cannot be updated" — entirely fixed text, no id or field name interpolated. The transformer's status is left untouched.
Emulator: the call SUCCEEDS — HTTP 200 with a normal UpdateTransformerResponse whose status is "inactive", and the stored transformer is actually deactivated (modifiedAt advanced).
Why: the emulator lets a client drive a state transition AWS has no path for, so a workflow that deactivates-then-reconfigures a transformer appears to work against the emulator and is rejected by AWS. Worse, state the emulator reports as mutable is permanently frozen on AWS.
Fix: UpdateTransformer's handler must gate every mutation on the stored transformer's status and refuse with the modeled ValidationException once it is active, without writing to the transformer state.
Traces: b2bi.update-transformer-status-toggle-active-inactive.

Cause 2 of 3: UpdateTransformer refuses a mapping change on an ACTIVE transformer with the wrong error class and message.
AWS: the same immutability rule covers unrelated members, not just status — changing `mapping` on an active transformer is refused with ValidationException (400, member `Message`), message "An active Transformer cannot be updated" (entirely fixed text), and the stored mapping is unchanged. This is state-gated, not blanket: the identical mapping change on the same transformer while it is still inactive succeeds (the check's positive control, which the emulator passes).
Emulator: refuses, but with ConflictException (HTTP 409, member `message`) rather than ValidationException (HTTP 400, member `Message`).
Why: the status code and the error-code discriminator both differ, so a client's error handling branches the wrong way — a ConflictException is conventionally retryable/transient, a ValidationException is a permanent caller error, so a retry loop will spin on a request AWS will never accept.
Fix: UpdateTransformer's immutability check must raise the modeled ValidationException with AWS's fixed text, not ConflictException.
Traces: b2bi.update-transformer-immutable-fields-once-active-rejected.

Cause 3 of 3: DeleteTransformer is a scaffolded stub, so dependency-order enforcement on delete does not exist.
AWS: Smithy DeleteTransformer takes transformerId (@length 1..64, pattern ^[a-zA-Z0-9_-]+$) and models ConflictException (HTTP 409, member `message`), documented as "thrown when you attempt to delete a resource ... that is being used by other resources". A transformer referenced by a capability's configuration.edi.transformerId is refused with ConflictException and the recorded message "Transformer config is currently in use by a Capability" — entirely fixed text — and the transformer remains readable via GetTransformer. Once the referencing capability is gone, the same DeleteTransformer succeeds (HTTP 200, empty body) and GetTransformer then raises ResourceNotFoundException with "Transformer not found with ID <transformerId>".
Emulator: every DeleteTransformer call answers HTTP 501 with error code `NotImplemented` and message "operation DeleteTransformer is scaffolded but not implemented in this emulator"; services/service-com-amazonaws-b2bi-b2bi/src/ops/delete_transformer.rs still carries the SCAFFOLDED marker and returns Scaffolded::not_implemented("DeleteTransformer").
Why: no b2bi teardown can complete, nothing can distinguish a live transformer from a deleted one, and the whole referenced-resource protection AWS provides on delete is absent. Note the already-filed wire issue 20261029T171003Z-b2bi-answers-200-where-aws-refuses-unknown-profile.md (cause 3) asks GetTransformer to refuse a transformer that DeleteTransformer removed — that fix is unreachable while DeleteTransformer cannot remove anything, so this cause gates it. This operation is NOT one of the stubs the suite auto-filed (UpdateCapability, UpdatePartnership, ListTransformers, TestConversion).
Fix: implement DeleteTransformer in services/service-com-amazonaws-b2bi-b2bi/src/ops/delete_transformer.rs — resolve the id (ResourceNotFoundException when absent), refuse with ConflictException while any capability references it, otherwise remove the record from the transformer state store — and delete the SCAFFOLDED marker line.
Traces: b2bi.delete-dependency-order-enforced-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 (23/42 passed; all 19 failures in b2bi).
Cause 1 — b2bi.update-transformer-status-toggle-active-inactive: after UpdateTransformer(status=active) succeeded and GetTransformer confirmed active, UpdateTransformer(transformerId=..., status="inactive") was expected to be refused with ValidationException 'An active Transformer cannot be updated' but SUCCEEDED, returning HTTP 200 and {'status': 'inactive', ..., 'modifiedAt': <advanced>} (ids/timestamps are per-run values).
Cause 2 — b2bi.update-transformer-immutable-fields-once-active-rejected: UpdateTransformer(mapping={templateLanguage: JSONATA, template: ...}) against an ACTIVE transformer — expected error code 'ValidationException', got 'ConflictException'. The inactive-state positive control in the same check passed, so the gate exists but raises the wrong class.
Cause 3 — b2bi.delete-dependency-order-enforced-rejected: DeleteTransformer(transformerId=<referenced by a capability>) — expected code 'ConflictException' with message 'Transformer config is currently in use by a Capability', got code 'NotImplemented' (HTTP 501, "operation DeleteTransformer is scaffolded but not implemented in this emulator"). services/service-com-amazonaws-b2bi-b2bi/src/ops/delete_transformer.rs lines 4 and 193 show the SCAFFOLDED marker and the not_implemented return.

## Steps to Reproduce
1. Clone the repo and boot the emulator from mainline. 2. Run the simparity-behavioral-aws-v1 behavioral suite. 3. Re-replay b2bi.update-transformer-status-toggle-active-inactive, b2bi.update-transformer-immutable-fields-once-active-rejected and b2bi.delete-dependency-order-enforced-rejected. 4. Cause 1: CreateTransformer, UpdateTransformer(status=active), then UpdateTransformer(status=inactive) — observe HTTP 200 and status 'inactive' where AWS returns 400 ValidationException 'An active Transformer cannot be updated'. 5. Cause 2: on an active transformer, UpdateTransformer with a new mapping — observe 409 ConflictException where AWS returns 400 ValidationException. 6. Cause 3: CreateTransformer, CreateCapability referencing it, then DeleteTransformer — observe HTTP 501 NotImplemented where AWS returns 409 ConflictException 'Transformer config is currently in use by a Capability' and leaves the transformer readable.
