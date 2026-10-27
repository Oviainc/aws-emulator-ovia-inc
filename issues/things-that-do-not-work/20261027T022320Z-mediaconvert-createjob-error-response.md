Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# CreateJob answers an empty Queue reference with NotFoundException, while the sibling handlers (and its own stated rule) make an empty reference a BadRequestException

_Filed: 2026-10-27T02:23:20Z_
_Operation: CreateJob | Class: error-response_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateJob applies the "a member the caller SENT is a value they meant" rule to `JobTemplate` — an empty/blank value is refused with `BadRequestException` — and its comment says this is the rule `CreateJobTemplate` applies to `Queue`. But CreateJob never applies it to its own `Queue` member: an empty or whitespace `Queue` falls through `queue_name_from_reference`, which returns the value unchanged, the store has no queue under that key, and the request is answered with `NotFoundException` (404, "The resource you specified in queue was not found.").

So the same blank value is a 400 on CreateJobTemplate/UpdateJobTemplate and a 404 on CreateJob, and on CreateJob the same member is a 400 when it is `jobTemplate` and a 404 when it is `queue`. A client cannot tell "I sent an empty queue" from "the queue I named was deleted".

## Evidence

services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs:224-234 states the rule and applies it only to `JobTemplate`:
```
        // "Optional. When you create a job, you can either specify a job template or
        // specify the transcoding settings individually." A member the caller SENT is a
        // value they meant, so an empty one is refused rather than quietly dropped, as
        // `CreateJobTemplate` refuses an empty `Queue`. ...
        if input.job_template.as_deref().map(str::trim) == Some("") {
            return Err(CreateJobErrorModeled::BadRequestException(BadRequestException {
                message: Some("JobTemplate must not be empty.".to_string()),
```
and create_job.rs:288-305 takes no such test for its own `Queue`:
```
            let queue = queue_reference
                .as_deref()
                .map(crate::state::queue_name_from_reference)
                .map(str::to_string)
                .unwrap_or_else(|| crate::state::DEFAULT_QUEUE_NAME.to_string());
            if !store.queues.contains_key(&queue) {
                return Err(CreateJobErrorModeled::NotFoundException(NotFoundException {
                    message: Some(crate::state::not_found_message("queue")),
```
(`queue_name_from_reference` -> `resource_name_from_reference`, src/state.rs:899-905, returns the reference unchanged when it is not an ARN, so `""`/`"   "` is looked up verbatim.)

The sibling rule it cites, services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job_template.rs:200-218:
```
        // ... and an EMPTY one is the modelled 400
        // rather than a value quietly dropped. ...
        let queue = match input.queue.as_deref().map(str::trim) {
            Some("") => {
                return Err(CreateJobTemplateErrorModeled::BadRequestException(
                    BadRequestException {
                        message: Some("Queue must not be empty.".to_string()),
```
and the same block in src/ops/update_job_template.rs:179-194.

Test coverage: tests/all/create_job_validation.rs:410-416 pins CreateJob's empty `jobTemplate` at 400/BadRequestException; tests/all/job_template_crud.rs:891-938 pins the empty `queue` at 400 for CreateJobTemplate and UpdateJobTemplate. No test sends an empty `queue` to CreateJob, so nothing pins the 404.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job(Role=R, Queue='', Settings={Inputs:[{FileInput:'s3://b/in.mp4'}], OutputGroups:[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}): observe HTTP 404 NotFoundException 'The resource you specified in queue was not found.'
3. Repeat with Queue='   ': same 404.
4. create_job(..., JobTemplate='') : HTTP 400 BadRequestException 'JobTemplate must not be empty.' — the same blank-member rule, a different code.
5. create_job_template(Name='t', Queue='', Settings=...): HTTP 400 BadRequestException 'Queue must not be empty.' — the sibling rule CreateJob's own comment cites.
