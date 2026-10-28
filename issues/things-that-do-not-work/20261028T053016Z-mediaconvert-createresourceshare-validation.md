Issue-Severity: LOW
Issue-Gap-Class: validation
Issue-Commit: 37884bada2128c91024def86659c4326af7ab1cb

# CreateResourceShare skips the crate's blank-reference test on JobId, answering 404 where every sibling reference answers 400

_Filed: 2026-10-28T05:30:16Z_
_Operation: CreateResourceShare | Class: validation_

> _Note: this issue was identified against the code base at commit `37884bada2128c91024def86659c4326af7ab1cb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The state module defines ONE blank test (`carries_no_characters`) for every member of this service that names a resource, and documents that no path may disagree about it. `CreateResourceShare` applies a blank test to `SupportCaseId` in its own handler, but passes `JobId` straight into `job_id_from_reference` with no such test, so a request carrying `JobId: "   "` (or `""`) is treated as a lookup of a job id nothing could hold and is refused with the modelled `NotFoundException` (404, "The resource you specified in jobId was not found.") instead of the `BadRequestException` (400, "JobId must not be empty.") the crate's one rule words for a reference that carries no characters. Every sibling reference member in the crate takes that test: `CreateJob`'s `Role`, `JobTemplate` and `Queue`, `CreateJobTemplate`/`UpdateJobTemplate`'s `Queue`, the nested `HopDestinations.Queue`, and `AssociateCertificate`/`DisassociateCertificate`'s `Arn`.

## Evidence

Rule (both halves quoted from this crate):
- services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs:590-597 — "Every member in this service that names a resource — the `Name` of a queue, a preset or a job template, and the references (`Queue`, `JobTemplate`, `Role`) that point at one — targets the model's bare `__string` ... Neither carries a resource identity: a key with no characters addresses nothing a later read can reach, and a reference with none resolves to nothing. ONE test for all of them, so no create or update path can disagree about which values are identities, answered with the bad request [`blank_member_message`] words." and src/state.rs:602-604 `pub fn carries_no_characters(value: &str) -> bool { value.trim().is_empty() }`.
- Sibling applying it to a reference member: src/ops/associate_certificate.rs:171-180 — "An `Arn` that carries no characters names no certificate, exactly as a blank name names no queue, and is refused naming the member it arrived in." + `if crate::state::carries_no_characters(&input.arn) { ... blank_member_message("Arn") }`.
- Sibling applying it to a job-template reference: src/ops/create_job.rs:229-238 — `if input.job_template.as_deref().is_some_and(crate::state::carries_no_characters) { ... blank_member_message("JobTemplate") }`.
- This handler: src/ops/create_resource_share.rs:160-171 refuses a blank sibling member — `if input.support_case_id.trim().is_empty() { ... BadRequestException ... }` — and then src/ops/create_resource_share.rs:176 resolves the reference with no blank test at all: `let job_id = crate::state::job_id_from_reference(&input.job_id);`, whose outcome is src/ops/create_resource_share.rs:193-201 `crate::state::ShareOutcome::NotFound => Err(... NotFoundException { message: Some(crate::state::not_found_message("jobId")) })`.
- `job_id_from_reference` does not normalise either (src/state.rs:939-941 -> `resource_name_from_reference`, src/state.rs:960-966 returns the reference unchanged when it is not an ARN), so the whitespace value reaches the `jobs` map probe verbatim.
Test coverage: tests/all/resource_share.rs exercises CreateResourceShare; tests/all/blank_resource_name.rs pins the blank-member 400 for the members that do take the test — no case covers a blank `JobId`.

## Steps to Reproduce

1. Boot the emulator and point a MediaConvert client at it.
2. CreateQueue + CreateJob to have at least one job stored (not strictly needed).
3. Call CreateResourceShare with JobId="   " (whitespace only) and SupportCaseId="case-1".
4. Observe HTTP 404 / NotFoundException "The resource you specified in jobId was not found.".
5. Compare with CreateResourceShare(JobId=<a real job id>, SupportCaseId="   "), which answers HTTP 400 / BadRequestException for the blank sibling member, and with CreateJob(JobTemplate="   ") / AssociateCertificate(Arn="   "), which both answer HTTP 400 / BadRequestException "<Member> must not be empty."
