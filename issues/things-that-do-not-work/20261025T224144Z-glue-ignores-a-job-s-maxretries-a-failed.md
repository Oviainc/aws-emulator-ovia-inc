Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__s3.glue-job-max-retries-two-produces-three-attempts-chained-by-previous-run-id
Issue-Key: glue__s3.glue-job-max-retries-two-produces-three-attempts-chained-by-previous-run-id|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Commit: 976c0fb9e67fe98b916554c76ebfa4c22f208dcd

# Glue ignores a job's MaxRetries: a failed run is never retried, so no Attempt/PreviousRunId retry chain is ever produced (1 cause)

> _Note: this issue was identified against the code base at commit `976c0fb9e67fe98b916554c76ebfa4c22f208dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
One root cause, grounded in one failing behavioral check of suite simparity-behavioral-ga-prod-v1.

Cause 1 of 1 — a job created with MaxRetries=N whose run ends FAILED is never retried: the emulator's job-run state machine produces exactly one JobRun per StartJobRun call, so the N additional attempts that AWS schedules, and the Attempt / PreviousRunId chain that identifies them, never exist.
Failing check ids for this cause: glue__s3.glue-job-max-retries-two-produces-three-attempts-chained-by-previous-run-id.

AWS: Smithy com.amazonaws.glue#Job.MaxRetries (target MaxRetries, integer, default 0) is documented as "The maximum number of times to retry this job after a JobRun fails"; the same member is accepted by CreateJob/UpdateJob and echoed by GetJob. com.amazonaws.glue#JobRun models exactly the two members that express the resulting chain: Attempt (target AttemptCount, integer, default 0, "The number of the attempt to run this job") and PreviousRunId (target IdString @length 1..255, "The ID of the previous run of this job. For example, the JobRunId specified in the StartJobRun action"). So for a job with MaxRetries=2 whose script exits non-zero, one StartJobRun must yield 1 + 2 = 3 JobRuns visible through GetJobRuns(JobName), all terminal and all FAILED, carrying consecutive Attempt values and each retry's PreviousRunId naming the immediately preceding attempt's Id; the original attempt carries no PreviousRunId. A job with MaxRetries=0 must yield one run, Attempt 0, and nothing may name that run as its PreviousRunId. The 0-based origin and the inter-attempt backoff are not stated by the model: the check pins the shape (count, consecutive attempts, single chain) and treats the origin value and the latency as the parts it measures from the target itself.

Emulator: the check never observes more than the single run StartJobRun minted. GetJobRuns(JobName) is polled for 1 + MaxRetries terminal runs and the condition is never satisfied, so the check hangs in that poll and the harness kills it ("Check timed out after 60.0s") instead of reaching the chain assertions. No retry run is created, and consequently no JobRun ever carries a non-zero Attempt or any PreviousRunId. Hypothesis: the hang is in the GetJobRuns retry-chain poll rather than in the preceding control leg, because the control leg only needs one run to reach a terminal state and this emulator terminates a job run immediately (see the related open issue below, which records a run settling on FAILED with ErrorMessage 'This emulator records job runs but executes no job script, so the job run ended without running.'); either way, within the budget the emulator produced no second attempt.

Why: a client cannot observe or rely on Glue's retry contract. Code that distinguishes a first attempt from a retry (Attempt), that walks a run's lineage (PreviousRunId / PredecessorRuns), that counts attempts to decide whether a job has exhausted MaxRetries, or that alerts only on the final attempt, all behave as if every job were MaxRetries=0. MaxRetries round-trips through GetJob as a stored value with no effect, which is worse than rejecting it: the job definition claims a retry policy the runtime does not implement.

Fix: the Glue job-run execution/state machine in services/service-com-amazonaws-glue-awsglue — the StartJobRun handler and the job-run state that terminates a run must, on a FAILED terminal attempt of a job whose MaxRetries > 0, schedule a further JobRun for the same job with Attempt incremented and PreviousRunId set to the attempt it retries, stopping after MaxRetries retries; GetJobRuns and GetJobRun must then serialize Attempt and PreviousRunId for those runs. Related open issues describe the same subsystem failing to EXECUTE a run (20261025T072821Z-glue-job-runs-are-terminated-failed-without-executing.md, 20261025T031335Z-glue-job-runs-never-execute-and-publish-no.md, 20261025T000830Z-glue-job-runs-are-recorded-but-never-executed.md) but none of them covers the retry policy or carries this check's replay key: a run that is terminated FAILED is precisely the input that must trigger a retry here, so this cause survives those fixes and is recorded separately.

Traces: glue__s3.glue-job-max-retries-two-produces-three-attempts-chained-by-previous-run-id.

Service-attribution note: the check's leading id token is the composite 'glue__s3' (SERVICES = ['glue','s3']). It fails on Glue's own job-run retry behavior, so it is filed under 'glue', which is bootstrapped.

Budget caveat for the fixer: the check's own header declares a budget well over 300s and asks for a per-suite timeout entry of 1800s for glue__s3; the harness gave it 60s. Re-replay it with that budget raised, so the chain assertions are reached rather than the poll being cut off again.

## Evidence
Expected (AWS): glue.create_job(Name=<subject>, Role=<glue-trust role>, Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<bucket>/scripts/fail.py'}, MaxCapacity=0.0625, GlueVersion='3.0', MaxRetries=2, Timeout=10, ExecutionProperty={'MaxConcurrentRuns':5}) with a script body that always exits non-zero; one glue.start_job_run(JobName=<subject>) then settles into THREE terminal JobRuns reported by the get_job_runs paginator, all with JobRunState 'FAILED', carrying Attempt values 0,1,2, the Attempt=0 run being the Id StartJobRun returned and carrying no PreviousRunId, and each Attempt=k run (k>0) carrying PreviousRunId == the Id of the Attempt=k-1 run; glue.get_job_run(JobName=<subject>, RunId=<last Id>) agrees on both Attempt and PreviousRunId. The control job created identically but with MaxRetries=0 yields a single run, Attempt 0, which nothing ever names as its PreviousRunId.

Actual (emulator): the poll over glue.get_job_runs(JobName=<subject>) never reaches 1+MaxRetries terminal runs — only the single run StartJobRun minted is ever reported — so the check never leaves that loop and the harness reports 'Check timed out after 60.0s'. The assertions it would otherwise have reported are: 'GetJobRuns(<subject>) reported 1 runs, expected 3 for MaxRetries=2: one original plus two retries' and, for the chain, 'the Attempt=1 run reports PreviousRunId=None'.

(Generated bucket/job/role names, run ids and timestamps differ per environment and are not part of the requirement; the requirement is the run COUNT 1+MaxRetries, the consecutive Attempt values, the FAILED terminal state of every attempt, and the PreviousRunId linkage between consecutive attempts.)

## Steps to Reproduce
1. s3.create_bucket on a fresh bucket and put_object a script at scripts/fail.py whose body is: import sys / print('sim2real deliberate failure', flush=True) / sys.exit(1).
2. iam.create_role with a trust policy for glue.amazonaws.com, attach arn:aws:iam::aws:policy/service-role/AWSGlueServiceRole, and put_role_policy granting s3:GetObject on arn:aws:s3:::<bucket>/* and s3:ListBucket on arn:aws:s3:::<bucket>.
3. glue.create_job twice against that role and script — a control with MaxRetries=0 and a subject with MaxRetries=2 — each with Command={'Name':'pythonshell','PythonVersion':'3.9','ScriptLocation':'s3://<bucket>/scripts/fail.py'}, MaxCapacity=0.0625, GlueVersion='3.0', Timeout=10, ExecutionProperty={'MaxConcurrentRuns':5}. Confirm glue.get_job reads MaxRetries back as 0 and 2 respectively.
4. glue.start_job_run(JobName=<subject>) once; record the returned JobRunId.
5. Poll the glue get_job_runs paginator for JobName=<subject> until every reported run is terminal. Observe exactly ONE run (the one from step 4), Attempt=0, no PreviousRunId; expect THREE runs with Attempt 0,1,2, all FAILED, chained by PreviousRunId.
6. glue.get_job_run(JobName=<subject>, RunId=<last attempt Id>) and observe that Attempt and PreviousRunId are absent/0 rather than naming the preceding attempt.
7. Re-replay check glue__s3.glue-job-max-retries-two-produces-three-attempts-chained-by-previous-run-id with the glue__s3 suite timeout raised to 1800s so the chain assertions are reached instead of the poll being cut off at 60s.
