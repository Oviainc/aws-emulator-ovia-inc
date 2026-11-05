Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name|run#11|AssertionError|assert declared == {"branchName": "feature/a", "stage": "PRODUCTION", "framework": "React",
Issue-Key: amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first|run#9|AssertionError|assert pending == "PENDING", f"a never-started job is {pending!r}"
Issue-Commit: 440ede2c1b46a54e9cd3a5bcdc414c48552fbe21

# amplify control plane: 2 net-new root causes — CreateBranch never derives displayName as a hostname label, and a reserved CreateDeployment job reports status CREATED where AWS reports PENDING

> _Note: this issue was identified against the code base at commit `440ede2c1b46a54e9cd3a5bcdc414c48552fbe21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBranch

## Description
Two net-new root causes in the amplify control plane, each grounded in a failing behavioral check. They are independent defects and must each be closed before this issue is submitted.

Note on existing issues: the open issue 20261102T085926Z-amplify-control-plane-8-net-new-root-causes.md carries both check ids below as replay keys, but its keys are bound to the assertions that used to fail (its Cause 2's `stage` default, and its Cause 7's GetJob 404). Those two have since been fixed — CreateBranch now reports stage 'NONE', and CreateDeployment now commits a job record that GetJob can read — so the suite no longer reports these two checks as owned by it, and the failures below are what the checks now fail on. The displayName derivation is described there as Cause 3 (which the replay could not reach at the time) and the reserved-job record as Cause 7, so a fixer should read those two causes alongside this issue; neither is closed by the fixes already landed.

Cause 1 of 2: CreateBranch echoes `branchName` into `displayName` instead of deriving a legal hostname label, so a branch name containing '/' yields a displayName that cannot be a domain prefix.
  AWS: Smithy models Branch$displayName (DisplayName, @required) and documents it as "The display name for the branch. This is used as the default domain prefix." The model states no derivation rule, so this is recorded behavior only: CreateBranch with no `displayName` produces a displayName equal to the branch name with every character that cannot appear in a DNS hostname label replaced by '-' — branchName 'feature/a' yields displayName 'feature-a', while a name already made only of label-legal characters (e.g. 'main') is reported unchanged. The request's other declared members round-trip verbatim (stage 'PRODUCTION', framework 'React', description, enableAutoBuild false), and `branchName` itself keeps the raw slashed form.
  Emulator: sets display_name = branch_name verbatim, so GetBranch(appId,'feature/a') reports displayName 'feature/a' — the slash is preserved. Replay: the branch record came back as {'branchName': 'feature/a', 'stage': 'PRODUCTION', 'framework': 'React', 'description': 'probe branch', 'enableAutoBuild': False, 'displayName': 'feature/a'}; only displayName diverges.
  Why: displayName IS the branch site's default domain prefix, so an un-derived value produces a default domain that cannot be addressed. A client that builds the branch URL from displayName emits a hostname label containing '/', i.e. a URL that is not resolvable, and any client that asserts on the derived prefix it recorded against AWS reads the raw name instead.
  Fix: the CreateBranch handler's derivation of `displayName` when the request omits it (and the Branch projection that reports it) — derive the label from branchName rather than cloning it.
  Traces: amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name; services/service-com-amazonaws-amplify-amplify/src/ops/create_branch.rs:261-266 (display_name = input.display_name.unwrap_or_else(|| input.branch_name.clone())).

Cause 2 of 2: the job CreateDeployment reserves is reported with status 'CREATED' where AWS reports 'PENDING', so the never-started manual deployment presents a status AWS never uses for it.
  AWS: Smithy models JobSummary$status @required -> enum JobStatus with values CREATED, PENDING, PROVISIONING, RUNNING, FAILED, SUCCEED, CANCELLING, CANCELLED; the model does not say which of CREATED/PENDING a reserved deployment carries, so this is recorded behavior only. The recording shows that the jobId CreateDeployment hands back is immediately readable through GetJob with status 'PENDING' (jobType 'MANUAL', empty steps) before StartDeployment is ever called, and StopJob then moves that job PENDING -> CANCELLED while ListJobs lists it newest-first above the branch's prior SUCCEED job and the branch's activeJobId stays on that prior job.
  Emulator: the reservation's JobSummary is built with crate::state::JOB_STATUS_CREATED, so GetJob on the id CreateDeployment just returned reports status 'CREATED'. Replay failed with AssertionError "a never-started job is 'CREATED'" at the GetJob poll, which also means the rest of the check (StopJob -> CANCELLED, ListJobs ordering, activeJobId) was never exercised and must be re-verified once the status is right.
  Why: a client distinguishes deployment phases by the status string. A poller that waits for PENDING before uploading, a UI that maps status to a phase, and any wait/terminal-status helper keyed on AWS's value set all see a status AWS does not emit for a reserved manual deployment, so they either hang or classify the job as an unknown state; 'CREATED' is a legal enum value, so the error surfaces as wrong control flow rather than a parse failure.
  Fix: the CreateDeployment handler's reserved-job projection — the reservation's JobSummary must carry the PENDING status, and the job-store/state status constant it reads must be the one GetJob, ListJobs and StopJob all report for a reserved, never-started deployment.
  Traces: amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first; services/service-com-amazonaws-amplify-amplify/src/ops/create_deployment.rs:346-372 (reserved_job_summary, status set from JOB_STATUS_CREATED at :362) and its commit into branch.jobs at :275-286.

## Evidence
Cause 1 (amplify.create-branch-applies-defaults-and-derives-a-hyphenated-display-name, both calls HTTP 200): after CreateBranch(appId=<app>, branchName='feature/a', stage='PRODUCTION', framework='React', description='probe branch', enableAutoBuild=False), GetBranch(appId,'feature/a')['branch'] is expected to carry displayName 'feature-a'. Actual: AssertionError "feature/a branch {'branchName': 'feature/a', 'stage': 'PRODUCTION', 'framework': 'React', 'description': 'probe branch', 'enableAutoBuild': False, 'displayName': 'feature/a'}" — every other member matches; only displayName is the raw, slash-bearing name. The earlier assertion in the same check (the 'main' branch defaults stage 'NONE', displayName 'main', enableAutoBuild True, ttl '5', enableBasicAuth False, enablePerformanceMode False, enablePullRequestPreview False, totalNumberOfJobs '0') now passes. The appId is a per-run value; the requirement is the '/'->'-' derivation.

Cause 2 (amplify.stop-job-cancels-a-pending-deployment-and-list-jobs-orders-newest-first, HTTP 200 both sides): after one completed manual deployment on branch 'main', CreateDeployment(appId,'main') returned jobId '2' (correct, the counter is not reused), and GetJob(appId,'main','2')['job']['summary']['status'] is expected to be 'PENDING'. Actual: 'CREATED' (AssertionError "a never-started job is 'CREATED'"). The job number '2' is a per-run value derived from the branch's history; the requirement is the status string. The downstream assertions of this check — StopJob leaving the job 'CANCELLED', ListJobs returning [('2','CANCELLED'), ('1','SUCCEED')] newest-first, and GetBranch's activeJobId still pointing at the prior SUCCEED job — were not reached and must be re-replayed after the status fix.

## Steps to Reproduce
Cause 1:
1. app = amplify.create_app(name=<unique>)['app'].
2. amplify.create_branch(appId=app['appId'], branchName='main'); amplify.get_branch(appId, 'main')['branch'] -> displayName must be 'main' (this holds today).
3. amplify.create_branch(appId=app['appId'], branchName='feature/a', stage='PRODUCTION', framework='React', description='probe branch', enableAutoBuild=False).
4. amplify.get_branch(appId, 'feature/a')['branch'] -> expect displayName 'feature-a' (the '/' derived to '-') while branchName stays 'feature/a'; observed displayName 'feature/a'.
5. amplify.list_branches(appId) -> branchNames must be ['feature/a', 'main'].

Cause 2:
1. Create an app and branch 'main'; run one full manual deployment (create_deployment -> PUT the zip to zipUploadUrl -> start_deployment) and poll get_job to SUCCEED (job '1').
2. dep = amplify.create_deployment(appId, 'main') -> jobId '2'.
3. amplify.get_job(appId, 'main', dep['jobId'])['job']['summary']['status'] -> expect 'PENDING'; observed 'CREATED'.
4. amplify.stop_job(appId, 'main', dep['jobId']) -> poll get_job to a terminal status; expect 'CANCELLED'.
5. amplify.list_jobs(appId, 'main') -> expect [('2','CANCELLED'), ('1','SUCCEED')] newest first; amplify.get_branch(appId,'main')['branch']['activeJobId'].lstrip('0') must still be '1'.
