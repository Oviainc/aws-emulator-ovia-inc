Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# Job template projections report Queue as a bare queue name instead of the queue ARN

_Filed: 2026-10-24T21:17:35Z_
_Operation: GetJobTemplate | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every projection of a `JobTemplate` (CreateJobTemplate 201, GetJobTemplate 200, ListJobTemplates 200, UpdateJobTemplate 200) reports `jobTemplate.queue` as the bare queue NAME, even when the request named the queue by ARN. Real MediaConvert reports a queue reference on a resource projection as the queue's ARN — this is already established for the sibling member `Job$Queue` by this repo's own recorded wire corpus (issue 20261022T124827Z-mediaconvert-resource-projections-omit-default-members-aws-always, cause 3: "job.queue is the full queue ARN (arn:aws:mediaconvert:<region>:<account>:queues/<name>), not the name the request sent"), and the emulator was fixed to do exactly that for jobs while the job-template projection still reports a name.

Expected (AWS): `POST /2017-08-29/jobTemplates` with `{"name":"t","queue":"tpl-queue",...}` answers 201 with `jobTemplate.queue == "arn:aws:mediaconvert:<region>:<account>:queues/tpl-queue"`, and the same ARN-valued member comes back from GetJobTemplate / ListJobTemplates / UpdateJobTemplate. A template created with the queue ARN round-trips that ARN.

Actual (emulator): `jobTemplate.queue == "tpl-queue"`. A template created with `queue: "arn:aws:mediaconvert:...:queues/tpl-queue"` is normalised DOWN to `"tpl-queue"`, so the value read back is not the value sent and is not parseable as an ARN.

Impact: a client that parses the queue reference off a template (extracting region/account, matching the template against a queue ARN it holds, tagging the queue, building an IAM resource) either fails to parse it or silently compares an ARN against a name and never matches — exactly the breakage that was fixed for `Job$Queue`. It also makes the two projections of the same concept disagree inside one service.

## Evidence

Spec/recorded grounding: Smithy `JobTemplate$Queue` (jsonName `queue`, "Optional. The queue that jobs created from this template are assigned to") is the same queue reference as `Job$Queue`, which the repo's own recorded corpus shows AWS projecting as a queue ARN (see the fixed issue 20261022T124827Z..., cause 3/4/5, and `JobRecord::view` in src/state.rs which now does `queue: Some(queue_arn(region, &self.queue))`). Console-exported MediaConvert job templates likewise carry `"Queue": "arn:aws:mediaconvert:<region>:<acct>:queues/Default"`.

Handler/state: services/service-com-amazonaws-mediaconvert-mediaconvert/src/state.rs, `JobTemplateRecord::view` (around line 1647) projects `queue: self.queue.clone()` — the stored NAME — while `JobRecord::view` (around line 2213) projects `queue: Some(queue_arn(region, &self.queue))`. The name is produced by `crate::state::queue_name_from_reference` in src/ops/create_job_template.rs (around line 216) and src/ops/update_job_template.rs (around line 192), which deliberately reduce an ARN the caller sent to a name before storing.

Test coverage status: the current (bare-name) behavior is asserted, so a fixer must update it — services/service-com-amazonaws-mediaconvert-mediaconvert/tests/all/job_template_crud.rs rows "a queue referenced by name" and "the same queue referenced by ARN" (lines ~163-184) both expect `"queue": FIXTURE_QUEUE`, and line ~498 compares `created.at("/jobTemplate/queue")`. No test asserts the ARN form. Note the recorded wire fixture for job templates (mediaconvert-job-template-lifecycle) created its template WITHOUT a Queue, so this member was never compared by the replay — which is why the earlier projection sweep did not catch it.

Secondary symptom of the same projection (same operation, same class): `jobTemplate.hopDestinations[].queue` is echoed back exactly as sent (a name stays a name, an ARN stays an ARN, see the "two hop destinations" row at job_template_crud.rs line ~203), so the same member is reported in two different forms depending on how the caller wrote it instead of being normalised to one.

## Steps to Reproduce

1. Boot the emulator and point a mediaconvert client at it (region us-west-2).
2. create_queue(Name='tpl-queue').
3. create_job_template(Name='t-by-name', Queue='tpl-queue', Settings={'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}).
4. Read the 201 body: expect JobTemplate.Queue == 'arn:aws:mediaconvert:us-west-2:<account>:queues/tpl-queue'; observe 'tpl-queue'.
5. get_job_template(Name='t-by-name') and the matching element of list_job_templates(): same expectation, same bare-name result.
6. create_job_template(Name='t-by-arn', Queue='arn:aws:mediaconvert:us-west-2:<account>:queues/tpl-queue', Settings=...): the ARN sent is not the value returned — the response carries 'tpl-queue'.
7. update_job_template(Name='t-by-name', Description='updated'): the returned template still carries the bare name.
8. Compare against create_job(Role=R, Queue='tpl-queue', Settings=...), whose Job.Queue IS the queue ARN — the two projections of the same reference disagree.
