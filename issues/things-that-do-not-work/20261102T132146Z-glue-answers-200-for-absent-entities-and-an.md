Issue-Source: trace-sweep-divergence
Issue-Trace: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id glue-gjwr-traj-create-job-stores-unvalidated-references glue-gjwr-traj-hand-written-job-definition-refusals
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|GetDatabase|4xx->2xx|$.Message
Issue-Key: glue|GetDatabase|4xx->2xx|$.__type
Issue-Key: glue|GetDatabase|4xx->2xx|status
Issue-Key: glue|GetConnection|4xx->2xx|$.Message
Issue-Key: glue|GetConnection|4xx->2xx|$.__type
Issue-Key: glue|GetConnection|4xx->2xx|status
Issue-Key: glue|GetJob|4xx->2xx|$.Message
Issue-Key: glue|GetJob|4xx->2xx|$.__type
Issue-Key: glue|GetJob|4xx->2xx|status
Issue-Key: glue|CreateJob|4xx->2xx|$.Message
Issue-Key: glue|CreateJob|4xx->2xx|$.__type
Issue-Key: glue|CreateJob|4xx->2xx|status
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# Glue answers 200 for absent entities and an invalid Glue version

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDatabase

## Description
Four Glue read/create paths answer HTTP 200 where AWS refuses with 400: three lookups resolve an entity that does not exist in the requested scope, and CreateJob accepts a GlueVersion that is not a real Glue version. Four causes, one shared shape: the existence/validity check is missing, so the success path is taken unconditionally.

Cause 1 of 4: GetDatabase resolves a database that exists only in another region. AWS: the Glue Data Catalog is regional — a database created in one region is not visible in another. Smithy GetDatabase models EntityNotFoundException (httpError 400); the recording shows 400 EntityNotFoundException with `Message` 'Database <name> not found.' where the fixed text is 'Database ' + the requested Name + ' not found.'. Emulator: 200 with a Database body. Why: a client probing regions to locate a catalog object finds it in every region, so region-scoped logic (and any cross-region guard) silently passes. Fix: the GetDatabase handler must scope the lookup to the request's region in catalog state. Traces: glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#2 (cluster also covers #task#3).

Cause 2 of 4: GetConnection resolves a connection that was never created. AWS: Smithy GetConnection models EntityNotFoundException (httpError 400); the recording shows 400 EntityNotFoundException with `Message` 'Connection with name <name> in account <accountId> is not found' — fixed text 'Connection with name ' + requested Name + ' in account ' + the caller's account id + ' is not found'. Emulator: 200. Why: CreateJob can reference a connection that does not exist and the follow-up read confirms it, so an unvalidated job reference is never surfaced. Fix: the GetConnection handler's existence check. Traces: glue-gjwr-traj-create-job-stores-unvalidated-references#task#10.

Cause 3 of 4: GetJob resolves a job that does not exist. AWS: Smithy GetJob models EntityNotFoundException (httpError 400); the recording shows 400 EntityNotFoundException with `Message` 'Job not found.' — entirely fixed text, no name interpolated. Emulator: 200 with a Job body. Why: a client cannot distinguish "job exists" from "job absent", so a create-then-verify sequence passes even when the create was refused. Fix: the GetJob handler's existence check. Traces: glue-gjwr-traj-hand-written-job-definition-refusals#task#14.

Cause 4 of 4: CreateJob accepts GlueVersion '9.9'. AWS: GlueVersion is pattern-valid but must name a Glue version Glue actually offers; Smithy CreateJob models InvalidInputException (httpError 400) and the recording shows 400 InvalidInputException with `Message` 'Glue version: <value> is invalid' — fixed text 'Glue version: ' + the requested GlueVersion + ' is invalid'. Emulator: 200, minting the job. Why: a typo in GlueVersion produces a job that can never run on real Glue but looks healthy against the emulator. Fix: CreateJob's GlueVersion validation (allow-list of offered versions, not just the regex). Traces: glue-gjwr-traj-hand-written-job-definition-refusals#task#3 (cluster also covers #task#5, #task#6, #task#8, #task#10).

Related open issues (do not cover these steps): 20261029T201859Z-glue-accepts-requests-aws-refuses-ten-create-delete.md and 20261102T114851Z-glue-4-net-new-root-causes-createjob-and.md.

## Evidence
Replay of trace set simparity-wire-aws-v1, service glue. Every cluster expected 400 and observed 200.

Cause 1 — glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#2 GetDatabase: status expected 400, got 200; $.__type missing (expected 'EntityNotFoundException'); $.Message missing (expected 'Database sim2real-probe-gjwr-db-3a0f88c9 not found.' — the name is the request's). 2 steps in cluster.
Cause 2 — glue-gjwr-traj-create-job-stores-unvalidated-references#task#10 GetConnection: status expected 400, got 200; $.__type missing (expected 'EntityNotFoundException'); $.Message missing (expected 'Connection with name sim2real-probe-gjwr-conn-3b26a480 in account 000000000000 is not found').
Cause 3 — glue-gjwr-traj-hand-written-job-definition-refusals#task#14 GetJob: status expected 400, got 200; $.__type missing (expected 'EntityNotFoundException'); $.Message missing (expected 'Job not found.').
Cause 4 — glue-gjwr-traj-hand-written-job-definition-refusals#task#3 CreateJob: status expected 400, got 200; $.__type missing (expected 'InvalidInputException'); $.Message missing (expected 'Glue version: 9.9 is invalid'). 5 steps in cluster.

## Steps to Reproduce
Replay the wire corpus and inspect these glue steps:
1. glue-gjwr-traj-catalog-is-regional-under-one-catalog-id#task#2 — GetDatabase in a region where the database was not created.
2. glue-gjwr-traj-create-job-stores-unvalidated-references#task#10 — GetConnection for a never-created connection.
3. glue-gjwr-traj-hand-written-job-definition-refusals#task#14 — GetJob for a job whose create was refused.
4. glue-gjwr-traj-hand-written-job-definition-refusals#task#3 — CreateJob with GlueVersion '9.9'.
Each answers 200 instead of 400 with the modeled Glue error.
