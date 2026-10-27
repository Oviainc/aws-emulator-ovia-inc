Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3938003190abafdb7ac5d97f1c3372e4bb8e6b5a

# CreateJobTemplate accepts a Name made only of whitespace, while CreateQueue and CreatePreset refuse one

_Filed: 2026-10-27T02:23:20Z_
_Operation: CreateJobTemplate | Class: validation_

> _Note: this issue was identified against the code base at commit `3938003190abafdb7ac5d97f1c3372e4bb8e6b5a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateJobTemplate tests the template `Name` with `is_empty()`, so only the zero-length string is refused: a name of `" "`, `"\t"` or `"   "` is stored as the template's key. The sibling creates in this same crate test the same member with `trim().is_empty()` and refuse every blank form with the modelled `BadRequestException`, and both of those rules are pinned by tests.

The handler's own comment states the rule it then fails to apply ("A nameless resource is the one thing that cannot be stored"), and the crate's state module asserts the trimmed rule as an invariant for queues. The result is that one service refuses a whitespace resource key on two of its three named resources and stores it on the third, leaving a job template whose key can only be addressed by percent-encoded whitespace.

## Evidence

services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job_template.rs:185-198:
```
        // The name is the resource's KEY and is stored exactly as the caller sent it:
        // ... A nameless resource is the one thing that cannot be stored, and that is the
        // modelled bad request.
        let name = input.name.clone();
        if name.is_empty() {
```

Sibling, services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_queue.rs:181-193:
```
        // ... The trim is only the test for a name that carries no characters at all, which the
        // model's bare `__string` does not refuse before this handler runs.
        let name = input.name;
        if name.trim().is_empty() {
```

Sibling, services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_preset.rs:162-172:
```
        // The `Name` is the resource's identity ... The trim is only the test for a name
        // that carries no characters at all ...
        let name = input.name;
        if name.trim().is_empty() {
```

Test coverage of the sibling rule (so the divergence is deliberate nowhere): tests/all/queue_name_identity.rs:226-263 `a_name_carrying_no_characters_is_a_bad_request` iterates `[("", ""), (" ", "%20"), ("\t", "%09"), ("\n", "%0A"), ("   ", "%20%20%20")]` and asserts HTTP 400 + BadRequestException for each; tests/all/preset_lifecycle.rs:497-510 has the rows "Name is only whitespace" (`"name": "   "`) and "Name is empty". No job-template test sends a blank `name` (tests/all/job_template_crud.rs:911 exercises blank `queue`, not blank `name`).

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it.
2. create_job_template(Name='   ', Settings={'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}): observe HTTP 201 and a stored template.
3. list_job_templates(ListBy='NAME'): the whitespace-named template is reported.
4. create_queue(Name='   ') and create_preset(Name='   ', Settings={'ContainerSettings':{'Container':'MP4'}}): both answer HTTP 400 BadRequestException for the same blank value.
5. GET /2017-08-29/jobTemplates/%20%20%20 still resolves the stored template, so the blank key is live state only one of the three creates allows.
