Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 5e76d9058893794719deef102d36e8b94e082214

# Virtual-cluster and job-run not-found refusals use emulator prose instead of the service's "<Resource> <id> doesn't exist." message

_Filed: 2026-10-24T18:50:50Z_
_Operation: DescribeVirtualCluster | Class: error-response_

> _Note: this issue was identified against the code base at commit `5e76d9058893794719deef102d36e8b94e082214`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: EMR on EKS builds its not-found refusal message from the requested id in the form `<Resource type> <id> doesn't exist.` — e.g. `Virtual cluster abc123 doesn't exist.` — the same form this crate already reproduces for job templates (`Job Template {id} doesn't exist.`) and security configurations (`Security Configuration {id} doesn't exist.`).

Actual: the virtual-cluster and job-run paths emit emulator-authored prose instead: DescribeVirtualCluster/UpdateVirtualCluster say `Virtual cluster {id} was not found`, DeleteVirtualCluster/ListJobRuns/ListManagedEndpoints say `Virtual cluster {id} was not found`, DescribeJobRun/CancelJobRun say `Job run {id} was not found in virtual cluster {vc}`. The HTTP status and error code are right; only the `message` text diverges, so a client (or trace comparison) that reads the message sees a different string than AWS returns.

## Evidence

Spec/established form: src/ops/describe_job_template.rs lines ~161-165 and src/ops/describe_security_configuration.rs lines ~164-170 document and implement the recorded AWS message form `<Resource type> <id> doesn't exist.` for this very service; the same service uses that form for virtual clusters and job runs. Handler: src/ops/describe_virtual_cluster.rs line ~163 (`"Virtual cluster {} was not found"`), src/ops/update_virtual_cluster.rs line ~176, src/ops/delete_virtual_cluster.rs line ~158, src/ops/describe_job_run.rs line ~157, src/ops/cancel_job_run.rs line ~158. Test coverage: tests assert only the status and error code (e.g. tests/all/virtual_cluster_job_run_loop.rs `refused(case, 400, "ResourceNotFoundException")`), never the message text.

## Steps to Reproduce

1. GET /virtualclusters/doesnotexist1
2. Observe 400 ResourceNotFoundException with message "Virtual cluster doesnotexist1 was not found" (expected: "Virtual cluster doesnotexist1 doesn't exist.")
3. Compare with GET /jobtemplates/doesnotexist1, which already answers "Job Template doesnotexist1 doesn't exist."
