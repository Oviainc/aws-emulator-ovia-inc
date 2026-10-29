Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: a19f71fb9b9d334f8efc64d442ea8394bd20139d

# DeleteQueue/DeletePreset/DeleteJobTemplate answer an empty-success 202 for a {Name} carrying no characters, skipping the crate's one blank-reference test

_Filed: 2026-10-28T15:06:52Z_
_Operation: DeleteQueue | Class: validation_

> _Note: this issue was identified against the code base at commit `a19f71fb9b9d334f8efc64d442ea8394bd20139d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The three named deletes take their `{Name}` straight from the path and probe the store with it. A value made only of whitespace (e.g. `DELETE /2017-08-29/queues/%20`) percent-decodes to `" "`, which names no resource at all, so the probe misses and the store reports `AlreadyAbsent`/`DeleteNamedOutcome::AlreadyAbsent` — and each handler answers that with the operation's empty 202 success. The caller is told the delete succeeded for a value that could never have been a resource key.

That skips the crate's ONE test for a caller-sent member that names a resource (`crate::state::carries_no_characters`), whose doc says explicitly that such a value must be "answered with the bad request `blank_member_message` words rather than with a lookup of a value nothing could hold", and that no door that mints or resolves such a member may disagree about it.

The sibling delete whose reference also arrives as a path label — `DisassociateCertificate` — DOES apply the test, and its comment names exactly this case ("it names no certificate, exactly as a blank name names no queue"); `tests/all/certificate_association.rs:338-347` pins the 400 for `" "`, `"   "`, `"\t"`, `"\n"` on both certificate doors. The three named deletes are the doors that disagree.

Nothing pins the current behaviour: `tests/all/blank_resource_name.rs` covers only the minting and reference-resolving doors (no DELETE case at all — grep for `DELETE`/`delete` in that file finds nothing), and no test in the suite sends a percent-encoded blank to a delete path.

## Evidence

The crate's stated rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-599`:
```
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — the `Name` of a queue, a preset or
/// a job template, and the references (`Queue`, `JobTemplate`, `Role`, the `JobId` a share
/// names, the `Arn` of a certificate or of a tagged resource) that point at one — targets
/// the model's bare `__string`, ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
```

The sibling that applies it to a path-label reference — `src/ops/disassociate_certificate.rs:176-186`:
```rust
        // `{Arn}` arrives percent-decoded from the path, so a value made only of
        // whitespace reaches here: it names no certificate, exactly as a blank name names
        // no queue, and it is refused naming the member it arrived in.
        if crate::state::carries_no_characters(&input.arn) {
            return Err(DisassociateCertificateErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(crate::state::blank_member_message("Arn")),
                },
```

The three doors that skip it:
* `src/ops/delete_queue.rs:158-170`:
```rust
        // `{Name}` is the queue name, as `GetQueue`'s is; this operation's input is not
        // documented to accept an ARN.
        let name = input.name;
        match state.delete_queue(&name) {
            ...
            crate::state::DeleteQueueOutcome::Deleted
            | crate::state::DeleteQueueOutcome::AlreadyAbsent => Ok(DeleteQueueOutput {}),
```
* `src/ops/delete_preset.rs:163-178`: `let name = input.name;` ... `DeleteNamedOutcome::Deleted | DeleteNamedOutcome::AlreadyAbsent => Ok(DeletePresetOutput {}),` — no `carries_no_characters` call anywhere in the file.
* `src/ops/delete_job_template.rs:151-173`: `let name = input.name;` ... `DeleteNamedOutcome::Deleted | DeleteNamedOutcome::AlreadyAbsent => Ok(DeleteJobTemplateOutput {}),` — likewise.

Test coverage: `tests/all/certificate_association.rs:338-347` pins the 400 for the certificate doors; `tests/all/blank_resource_name.rs` (whole file) exercises creates, hop destinations, `CreateResourceShare$JobId` and the three tagging `Arn`s but no DELETE path, so no existing assertion changes.

## Steps to Reproduce

1. Send DELETE /2017-08-29/queues/%20 (a single space as the {Name} label).
2. Observe HTTP 202 with an empty body — the service reports a successful delete for a name that carries no characters.
3. Send DELETE /2017-08-29/certificates/%20 for comparison: HTTP 400 BadRequestException with message "Arn must not be empty."
4. Repeat step 1 against DELETE /2017-08-29/presets/%20 and DELETE /2017-08-29/jobTemplates/%20 — both answer 202 as well.
