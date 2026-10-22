Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-job-submission-workflow
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-job-submission-workflow mediaconvert-queue-lifecycle mediaconvert-job-template-lifecycle
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|CreateQueue|2xx->2xx|$.queue.concurrentJobs
Issue-Key: mediaconvert|CreateQueue|2xx->2xx|$.queue.maximumConcurrentFeeds
Issue-Key: mediaconvert|GetQueue|2xx->2xx|$.queue.concurrentJobs
Issue-Key: mediaconvert|GetQueue|2xx->2xx|$.queue.maximumConcurrentFeeds
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.accelerationSettings
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.billingTagsSource
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.messages
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.priority
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.queue
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.shareStatus
Issue-Key: mediaconvert|CreateJob|2xx->2xx|$.job.statusUpdateInterval
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.accelerationSettings
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.billingTagsSource
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.messages
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.priority
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.queue
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.shareStatus
Issue-Key: mediaconvert|GetJob|2xx->2xx|$.job.statusUpdateInterval
Issue-Key: mediaconvert|ListJobs|2xx->2xx|$.jobs[arn="arn
Issue-Key: mediaconvert|CreateJobTemplate|2xx->2xx|$.jobTemplate.accelerationSettings
Issue-Key: mediaconvert|CreateJobTemplate|2xx->2xx|$.jobTemplate.statusUpdateInterval
Issue-Key: mediaconvert|GetJobTemplate|2xx->2xx|$.jobTemplate.accelerationSettings
Issue-Key: mediaconvert|GetJobTemplate|2xx->2xx|$.jobTemplate.statusUpdateInterval
Issue-Key: mediaconvert|ListJobTemplates|2xx->2xx|$.jobTemplates[arn="arn
Issue-Key: mediaconvert|UpdateJobTemplate|2xx->2xx|$.jobTemplate.accelerationSettings
Issue-Key: mediaconvert|UpdateJobTemplate|2xx->2xx|$.jobTemplate.statusUpdateInterval
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaConvert resource projections omit default members AWS always returns (Queue, Job, JobTemplate)

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
This issue bundles 9 distinct net-new WIRE root causes in the MediaConvert emulator's response projections: every read and write path returns a resource body that is MISSING members AWS populates unconditionally, and Job.queue is projected as a bare name instead of a queue ARN. All 9 must be closed before submitting. Each cause is one divergence cluster of the simparity-wire-aws-v1 replay; the HTTP status is correct in every one of them, only the body diverges.

The two open behavioral issues (20261022T040904Z-mediaconvert-queue-job-control-plane-4-fidelity-divergences.md and 20261022T110844Z-mediaconvert-job-queue-plane-5-fidelity-divergences-updatequeue.md) do NOT cover any cause below: they concern idempotency, role trust, the Default SYSTEM queue, enum-rejection message text, UpdateQueue being a stub, JobTemplate settings merge, PAUSED queues and Job.ErrorCode. None of them is about these response-member omissions.

=== Cause 1 of 9: CreateQueue's response Queue omits concurrentJobs and maximumConcurrentFeeds ===
AWS: Smithy: CreateQueue is POST /2017-08-29/queues with success code 201 and output member Queue -> the Queue shape, which carries ConcurrentJobs (__integer, jsonName 'concurrentJobs', doc 'The maximum number of jobs your queue can process concurrently') and MaximumConcurrentFeeds (__integerMin0, jsonName 'maximumConcurrentFeeds', range min 0). Recorded behavior: the 201 Queue body always carries both members — an account-default concurrency value for concurrentJobs and 0 for maximumConcurrentFeeds — even when the request supplies neither.
Emulator: status 201 and the rest of the Queue body agree, but both members are absent from the JSON (replay reports '$.queue.concurrentJobs: missing' and '$.queue.maximumConcurrentFeeds: missing, expected 0').
Why: a client that reads the created queue's concurrency (capacity planning, asserting the queue it just provisioned, echoing the queue into its own config) gets a missing key where AWS always supplies an integer; typed SDK consumers see null/None for a member that is never absent on AWS, and code that compares a requested concurrency against the created one cannot.
Fix: the CreateQueue handler's Queue projection and the queue record in the service state — give the queue record a concurrent-jobs and maximum-concurrent-feeds field with AWS's defaults when the request omits them, and project both in the Queue view used by the response.
Traces: mediaconvert-job-submission-workflow#setup#2 (cluster also covers mediaconvert-queue-lifecycle#setup#1).

=== Cause 2 of 9: GetQueue's response Queue omits the same two members ===
AWS: Smithy: GetQueue is GET /2017-08-29/queues/{Name} -> 200 with output Queue. Recorded behavior: the read-back Queue carries concurrentJobs (recorded value 200 for an on-demand queue) and maximumConcurrentFeeds 0.
Emulator: 200 with a Queue body in which both members are missing.
Why: the same members are unreadable on the read path, so a client cannot discover a queue's concurrency at all — not on create and not on get.
Fix: the Queue projection shared by GetQueue/ListQueues in the service state's queue view (same projection as cause 1).
Traces: mediaconvert-queue-lifecycle#task#2.

=== Cause 3 of 9: CreateJob's response Job omits six always-populated members and projects Job.queue as a bare queue NAME instead of the queue ARN ===
AWS: Smithy: CreateJob is POST /2017-08-29/jobs -> 201 with output Job. The Job shape carries BillingTagsSource (BillingTagsSource enum: QUEUE|PRESET|JOB_TEMPLATE|JOB), AccelerationSettings (structure with required Mode), StatusUpdateInterval (StatusUpdateInterval enum, includes SECONDS_60), Priority (__integerMinNegative50Max50), Messages (JobMessages), ShareStatus (ShareStatus enum: NOT_SHARED|INITIATED|SHARED) and Queue (jsonName 'queue'). Recorded behavior: the 201 Job always carries billingTagsSource 'JOB', accelerationSettings {mode: 'DISABLED'}, statusUpdateInterval 'SECONDS_60', priority 0, messages {info: [], warning: []} and shareStatus 'NOT_SHARED' as service-applied defaults when the request omits them, and job.queue is the full queue ARN (arn:aws:mediaconvert:<region>:<account>:queues/<name>), not the name the request sent.
Emulator: 201, but all six members are absent and job.queue echoes the request's bare queue name ('<queue-name>' instead of 'arn:aws:mediaconvert:<region>:<account>:queues/<queue-name>').
Why: two separate client breakages. (a) A client that branches on acceleration, status-update cadence, priority or share status reads nothing where AWS always has a value, and job.messages is a required-shaped container that code iterates without a null check. (b) job.queue is an ARN on AWS, so anything that parses it — extracting the account/region, tagging, IAM resource construction, matching the job against a queue ARN held elsewhere — either fails to parse or silently compares an ARN against a name and never matches.
Fix: the CreateJob handler's Job projection and the job record in the service state — apply AWS's defaults for the six members when the request omits them, and store/project the job's queue as a queue ARN built from the resolved queue (keeping the request's name only as the lookup key).
Traces: mediaconvert-job-submission-workflow#task#3.

=== Cause 4 of 9: GetJob returns the same defective Job projection ===
AWS: Smithy: GetJob is GET /2017-08-29/jobs/{Id} -> 200 with output Job (same Job shape as cause 3). Recorded behavior: the read-back Job carries the same six members and an ARN-valued queue.
Emulator: 200 with the six members missing and queue projected as the bare name.
Why: as cause 3, on the read path a client polls — so every poll of a job's state is missing the members and carries an unparseable queue value.
Fix: the shared Job view in the service state (the projection GetJob serializes), so create and read agree.
Traces: mediaconvert-job-submission-workflow#task#4.

=== Cause 5 of 9: ListJobs returns the same defective Job projection for every listed job ===
AWS: Smithy: ListJobs is GET /2017-08-29/jobs -> 200 with output Jobs (__listOfJob) + NextToken. Recorded behavior: each listed Job carries billingTagsSource, accelerationSettings, statusUpdateInterval, priority, messages, shareStatus and an ARN-valued queue.
Emulator: 200 with the correct job set, but each element omits the six members and carries the bare queue name.
Why: list is how clients reconcile state in bulk (dashboards, cleanup, queue attribution). Attributing listed jobs to a queue by ARN finds nothing, and per-job defaults are missing across the whole page.
Fix: same shared Job view as cause 4, used by the ListJobs serializer.
Traces: mediaconvert-job-submission-workflow#task#5.

=== Cause 6 of 9: CreateJobTemplate's response JobTemplate omits accelerationSettings and statusUpdateInterval ===
AWS: Smithy: CreateJobTemplate is POST /2017-08-29/jobTemplates -> 201 with output JobTemplate; the JobTemplate shape carries AccelerationSettings and StatusUpdateInterval (enum). Recorded behavior: the 201 body always carries accelerationSettings {mode: 'DISABLED'} and statusUpdateInterval 'SECONDS_60' when the request omits them (alongside priority 0 and hopDestinations [], which the emulator does project).
Emulator: 201 with both members absent.
Why: a client that reads back the template it just created to confirm its acceleration/cadence configuration, or that round-trips a template body into a later Create/Update call, loses two members AWS always sets — and a round-trip that re-sends the read body silently drops them.
Fix: the CreateJobTemplate handler's JobTemplate projection and the job-template record in the service state — default and project both members.
Traces: mediaconvert-job-template-lifecycle#setup#1.

=== Cause 7 of 9: GetJobTemplate returns the same two members missing ===
AWS: Smithy: GetJobTemplate -> 200 with output JobTemplate (same shape). Recorded behavior: accelerationSettings {mode: 'DISABLED'} and statusUpdateInterval 'SECONDS_60' present.
Emulator: 200 with both missing, both before and after an update.
Why: as cause 6 on the read path, which is what clients inspect templates with.
Fix: the shared JobTemplate view in the service state.
Traces: mediaconvert-job-template-lifecycle#task#2 (cluster also covers mediaconvert-job-template-lifecycle#task#6).

=== Cause 8 of 9: ListJobTemplates returns the same two members missing for every listed template ===
AWS: Smithy: ListJobTemplates is GET /2017-08-29/jobTemplates -> 200 with output JobTemplates (__listOfJobTemplate). Recorded behavior: each listed template carries both members.
Emulator: 200 with the right template set, each element missing accelerationSettings and statusUpdateInterval.
Why: bulk inspection of templates cannot see their acceleration/cadence settings at all.
Fix: the shared JobTemplate view used by the ListJobTemplates serializer.
Traces: mediaconvert-job-template-lifecycle#task#3.

=== Cause 9 of 9: UpdateJobTemplate's response JobTemplate omits the same two members ===
AWS: Smithy: UpdateJobTemplate is PUT /2017-08-29/jobTemplates/{Name} -> 200 with output JobTemplate. Recorded behavior: the mutated template is returned with accelerationSettings {mode: 'DISABLED'} and statusUpdateInterval 'SECONDS_60' preserved.
Emulator: 200, the update itself applies, but both members are absent from the returned template.
Why: a client that uses the update response as the new authoritative copy of the template (the normal read-after-write shortcut) ends up with a body that lost two members, which then propagates into whatever it stores or re-sends.
Fix: the UpdateJobTemplate handler must serialize the same JobTemplate view as causes 6-8, with both members preserved across the mutation.
Traces: mediaconvert-job-template-lifecycle#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 62 steps, 27 passed, score 43.5%. bootstrap_signal=false, emulator_up=true, state_reset_failures=0, so these are genuine per-step body divergences, not a boot or state-bleed artifact. Status matches expectation in every cause below; only the response body diverges.

Cause 1 - mediaconvert-job-submission-workflow#setup#2, CreateQueue, status 201 == 201 [mismatch]
  Expected: $.queue.concurrentJobs present (integer), $.queue.maximumConcurrentFeeds == 0
  Actual:   both members missing (None). Same cluster: mediaconvert-queue-lifecycle#setup#1.

Cause 2 - mediaconvert-queue-lifecycle#task#2, GetQueue, status 200 == 200 [mismatch]
  Expected: $.queue.concurrentJobs == 200, $.queue.maximumConcurrentFeeds == 0
  Actual:   both members missing (None).

Cause 3 - mediaconvert-job-submission-workflow#task#3, CreateJob, status 201 == 201 [mismatch]
  Expected: $.job.queue == 'arn:aws:mediaconvert:<region>:<account>:queues/<queue-name>'; $.job.billingTagsSource == 'JOB'; $.job.accelerationSettings == {'mode': 'DISABLED'}; $.job.statusUpdateInterval == 'SECONDS_60'; $.job.priority == 0; $.job.messages == {'info': [], 'warning': []}; $.job.shareStatus == 'NOT_SHARED'
  Actual:   $.job.queue == '<queue-name>' (bare name, no ARN); the other six members missing (None).

Cause 4 - mediaconvert-job-submission-workflow#task#4, GetJob, status 200 == 200 [mismatch]
  Expected/Actual: identical to cause 3 ($.job.queue bare name; the six members missing).

Cause 5 - mediaconvert-job-submission-workflow#task#5, ListJobs, status 200 == 200 [mismatch]
  Expected/Actual: identical to cause 3, per listed element ($.jobs[arn="arn:aws:mediaconvert:<region>:<account>:jobs/<id>"].queue bare name; the six members missing).

Cause 6 - mediaconvert-job-template-lifecycle#setup#1, CreateJobTemplate, status 201 == 201 [mismatch]
  Expected: $.jobTemplate.accelerationSettings == {'mode': 'DISABLED'}, $.jobTemplate.statusUpdateInterval == 'SECONDS_60'
  Actual:   both missing (None).

Cause 7 - mediaconvert-job-template-lifecycle#task#2, GetJobTemplate, status 200 == 200 [mismatch]
  Expected/Actual: as cause 6. Same cluster: mediaconvert-job-template-lifecycle#task#6.

Cause 8 - mediaconvert-job-template-lifecycle#task#3, ListJobTemplates, status 200 == 200 [mismatch]
  Expected/Actual: as cause 6, on the listed element.

Cause 9 - mediaconvert-job-template-lifecycle#task#5, UpdateJobTemplate, status 200 == 200 [mismatch]
  Expected/Actual: as cause 6, on the mutated template returned by the update.

Model citations used: Smithy CreateQueue (POST /2017-08-29/queues, 201, output Queue), GetQueue (GET /2017-08-29/queues/{Name}, 200), Queue (ConcurrentJobs __integer 'concurrentJobs', MaximumConcurrentFeeds __integerMin0 'maximumConcurrentFeeds', Arn, Status, Type, SubmittedJobsCount, ProgressingJobsCount); CreateJob (POST /2017-08-29/jobs, 201, output Job), GetJob (GET /2017-08-29/jobs/{Id}, 200), ListJobs (GET /2017-08-29/jobs, 200, Jobs -> __listOfJob), Job (Queue 'queue', BillingTagsSource enum QUEUE|PRESET|JOB_TEMPLATE|JOB, AccelerationSettings{Mode required}, StatusUpdateInterval enum incl. SECONDS_60, Priority -50..50, Messages -> JobMessages, ShareStatus enum NOT_SHARED|INITIATED|SHARED); CreateJobTemplate (POST /2017-08-29/jobTemplates, 201), UpdateJobTemplate (PUT /2017-08-29/jobTemplates/{Name}, 200), ListJobTemplates (GET /2017-08-29/jobTemplates, 200), JobTemplate (AccelerationSettings, StatusUpdateInterval, Priority, HopDestinations, Type). The DEFAULT VALUES themselves (concurrentJobs, maximumConcurrentFeeds 0, billingTagsSource JOB, mode DISABLED, SECONDS_60, priority 0, empty messages, NOT_SHARED) and the ARN-valued job.queue are recorded behavior only - the model declares the members but not their service-applied defaults.

## Steps to Reproduce
Boot the emulator and point a botocore mediaconvert client at it (region us-west-2), then replay the fixtures named per cause with replay_wire_corpus, or reproduce by hand:

Causes 1-2: create_queue(Name=Q) with no ConcurrentJobs/MaximumConcurrentFeeds. Expect the 201 Queue to carry 'ConcurrentJobs' (integer) and 'MaximumConcurrentFeeds' == 0; observe both keys absent. Then get_queue(Name=Q) - same two keys must be present (recorded ConcurrentJobs 200); observe both absent.

Causes 3-5: create_queue(Name=Q); create_job(Role=R, Queue=Q, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}],'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}) supplying none of Priority/AccelerationSettings/StatusUpdateInterval/BillingTagsSource. Expect the 201 Job to carry BillingTagsSource 'JOB', AccelerationSettings {'Mode':'DISABLED'}, StatusUpdateInterval 'SECONDS_60', Priority 0, Messages {'Info':[],'Warning':[]}, ShareStatus 'NOT_SHARED', and Job.Queue == the queue's ARN; observe the six keys absent and Job.Queue == 'Q'. Repeat the assertions on get_job(Id=<job id>) and on the matching element of list_jobs(Queue=Q)['Jobs'].

Causes 6-9: create_job_template(Name=T, Settings={'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}) with no AccelerationSettings/StatusUpdateInterval. Expect the 201 JobTemplate to carry AccelerationSettings {'Mode':'DISABLED'} and StatusUpdateInterval 'SECONDS_60'; observe both absent. Assert the same on get_job_template(Name=T), on the element of list_job_templates()['JobTemplates'], and on the JobTemplate returned by update_job_template(Name=T, Description='updated').
