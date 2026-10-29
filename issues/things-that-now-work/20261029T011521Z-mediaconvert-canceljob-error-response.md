Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36

# CancelJob answers a job `{Id}` carrying no characters with NotFoundException, where the crate's one blank-reference rule (and CreateResourceShare, on the same job reference) answers BadRequestException

_Filed: 2026-10-29T01:15:21Z_
_Operation: CancelJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `41c43c88bb67cbcbcf61b56c50a2a3e4a7862d36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CancelJob` takes `{Id}` straight from the path and probes the job store with it. A label made only of whitespace (`DELETE /2017-08-29/jobs/%20`) percent-decodes to `" "`, the store misses, and the handler answers the modelled `NotFoundException` (404) worded "The resource you specified in id was not found." — reporting a missing RESOURCE for a value that could never have been a job id.

That skips the crate's ONE test for a caller-sent member that names a resource (`crate::state::carries_no_characters`), whose own doc says such a value must be "answered with the bad request `blank_member_message` words rather than with a lookup of a value nothing could hold", and that "no door that mints or resolves such a member can disagree about which values are identities".

The door that disagrees with it is one of a PAIR on the same reference: `CreateResourceShare` resolves the very same thing — a MediaConvert job id (by id or by ARN) — and DOES apply the test, refusing with 400 "JobId must not be empty." and stating in its comment that such a value is refused "never as a lookup of an id nothing could hold". `DisassociateCertificate` shows the test is reachable and expected for a value that arrives as a PATH LABEL. `GetJob` (`src/ops/get_job.rs:164-170`) reads its `{Id}` the same way as `CancelJob` and should be aligned with it in the same change.

Distinct from the open issue `20261028T150652Z-mediaconvert-deletequeue-validation`: that one is about the three named deletes answering an empty-SUCCESS 202 for a blank `{Name}`; this is the job-id doors answering a resource-not-found 404 for a blank `{Id}`, and neither the operation nor the symptom overlaps.

## Evidence

The crate's stated rule — `services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:588-599`:
```
/// Whether a string member the caller SENT carries no characters at all.
///
/// Every member in this service that names a resource — ... and the references (`Queue`,
/// `JobTemplate`, `Role`, the `JobId` a share names, the `Arn` of a certificate or of a
/// tagged resource) that point at one — ... ONE test for all
/// of them, so no door that mints or resolves such a member can disagree about which
/// values are identities, answered with the bad request [`blank_member_message`] words
/// rather than with a lookup of a value nothing could hold.
```

The sibling door on the SAME reference (a job id) — `src/ops/create_resource_share.rs:155-169`:
```rust
        // "Specify MediaConvert Job ID or ARN to share": a value carrying no characters is
        // neither an id nor an ARN, so it names no job and resolves to nothing. That is the
        // test every resource-naming member in this service takes
        // (`crate::state::carries_no_characters`), so it is refused as a problem in the
        // request naming the member it arrived in, never as a lookup of an id nothing could
        // hold.
        if crate::state::carries_no_characters(&input.job_id) {
            return Err(CreateResourceShareErrorModeled::BadRequestException(
                BadRequestException {
                    message: Some(crate::state::blank_member_message("JobId")),
```

That a whitespace path label really reaches a handler — `src/ops/disassociate_certificate.rs:176-186`:
```rust
        // `{Arn}` arrives percent-decoded from the path, so a value made only of
        // whitespace reaches here: it names no certificate, exactly as a blank name names
        // no queue, and it is refused naming the member it arrived in.
        if crate::state::carries_no_characters(&input.arn) {
```

The door that skips it — `src/ops/cancel_job.rs:151-169` (no `carries_no_characters` call anywhere in the file):
```rust
        match state.cancel_job(&input.id, now) {
            ...
            crate::state::CancelOutcome::NotFound => {
                Err(CancelJobErrorModeled::NotFoundException(NotFoundException {
                    // The refusal names the request member the lookup read, `Id`.
                    message: Some(crate::state::not_found_message("id")),
```
and the store it reaches, `src/state.rs:4753-4756`:
```rust
    pub fn cancel_job(&mut self, id: &str, now: SystemTime) -> CancelOutcome {
        let Some(job) = self.jobs.get_mut(id) else {
            return CancelOutcome::NotFound;
        };
```
The read-side door with the identical shape — `src/ops/get_job.rs:164-170`: `let Some(job) = state.jobs.get(&input.id) else { ... not_found_message("id") ... }`.

Test coverage: `tests/all/cancel_job_idempotence.rs:3-11` pins the 404 only for "a job this account does not hold" (a well-formed id), which is a different case from a value that names nothing at all; `tests/all/blank_resource_name.rs` (whole file) covers the named creates, hop destinations, `CreateResourceShare$JobId` and the tagging `Arn`s and sends no blank `{Id}` to `CancelJob` or `GetJob`, so no existing assertion changes.

## Steps to Reproduce

1. Send DELETE /2017-08-29/jobs/%20 (a single space as the {Id} label).
2. Observe HTTP 404 NotFoundException with message "The resource you specified in id was not found." — a missing-resource answer for a value that carries no characters.
3. For comparison, POST /2017-08-29/resourceShares with {"jobId":" ","supportCaseId":"case-1"}: HTTP 400 BadRequestException "JobId must not be empty.", the refusal the crate's one blank-reference rule requires for the same job reference.
4. Send GET /2017-08-29/jobs/%20 — observe the same 404 from the read-side door.
