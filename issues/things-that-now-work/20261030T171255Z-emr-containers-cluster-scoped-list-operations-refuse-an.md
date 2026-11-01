Issue-Source: trace-sweep-divergence
Issue-Trace: emr-containers-managed-endpoint-job-run-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-containers-managed-endpoint-job-run-error-paths
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers|ListManagedEndpoints|2xx->4xx|$.endpoints
Issue-Key: emr-containers|ListManagedEndpoints|2xx->4xx|status
Issue-Key: emr-containers|ListJobRuns|2xx->4xx|$.jobRuns
Issue-Key: emr-containers|ListJobRuns|2xx->4xx|status
Issue-Commit: 5e13030fba3716cb0d2a1354c811e28788b6a350

# emr-containers cluster-scoped list operations refuse an unknown virtualClusterId with HTTP 400 instead of an empty 200 page

> _Note: this issue was identified against the code base at commit `5e13030fba3716cb0d2a1354c811e28788b6a350`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedEndpoints

## Description
Two net-new root causes in the emr-containers emulator, both the same defect: a list operation whose collection is scoped to a virtual cluster by the `{virtualClusterId}` path label treats an id that resolves to nothing as a request error and refuses with HTTP 400, where AWS answers HTTP 200 with an empty collection. Both causes must be fixed and both fixture steps must replay clean before this issue is closed.

Common model evidence: both operations model exactly `InternalServerException` (500) and `ValidationException` (400) - no not-found error is bound to either, which is consistent with the recorded 200: the absence of the scoping cluster is not an error condition on these read paths. Both are `@paginated` (inputToken/outputToken nextToken, pageSize maxResults) with the items member marked as the paginated `items`, so the items member is the page body a paginating client walks.

Cause 1 of 2 - ListManagedEndpoints (GET /virtualclusters/{virtualClusterId}/endpoints).
AWS: Smithy: ListManagedEndpoints.virtualClusterId = ResourceIdString, @required @httpLabel, @length 1..64, @pattern ^[0-9a-z]+$; output members `endpoints` (Endpoints list, the paginated items) and `nextToken`; errors InternalServerException(500) and ValidationException(400) only. Recorded behavior: for a syntactically valid id that names no virtual cluster, HTTP 200 with `endpoints: []` and no nextToken.
Emulator: HTTP 400 and a body with no `endpoints` member at all.
Why: any client that enumerates endpoints before deciding whether a cluster has any - a reconciler, a teardown sweep, a paginator - gets an exception instead of an empty page. A generated SDK paginator raises on the first page rather than yielding zero items, so the caller's "nothing to do" branch is never reached and the loop aborts. Code written against AWS also cannot use the empty page as the "cluster is unknown or empty" signal, because the emulator turns that into a hard failure.
Fix: the handler/state lookup for ListManagedEndpoints in the emr-containers service crate - resolving `virtualClusterId` must not gate the response: when the id is well formed but names no cluster, serialize the normal 200 output with an empty `endpoints` list (and no nextToken) rather than raising. Keep the ValidationException for an id that violates the ResourceIdString @length/@pattern constraint.
Traces: fixture emr-containers-managed-endpoint-job-run-error-paths step task#2.

Cause 2 of 2 - ListJobRuns (GET /virtualclusters/{virtualClusterId}/jobruns).
AWS: Smithy: ListJobRuns.virtualClusterId carries the identical ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$ constraint; output members `jobRuns` (JobRuns list, the paginated items) and `nextToken`; the same two errors only. Recorded behavior: for a valid id naming no virtual cluster, HTTP 200 with `jobRuns: []`.
Emulator: HTTP 400 and a body with no `jobRuns` member at all.
Why: same as Cause 1, and worse for job polling: a monitor that lists job runs for a cluster id it holds (one that has been deleted, or that it is about to create) sees a refusal rather than an empty result and treats a legitimate empty state as an outage.
Fix: the handler/state lookup for ListJobRuns in the emr-containers service crate - same change: do not refuse on an unresolvable scoping id, return the empty 200 page. The two list handlers should share one "scope resolves to nothing -> empty page" helper so they cannot drift.
Traces: fixture emr-containers-managed-endpoint-job-run-error-paths step task#4.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator built from this checkout (35 steps, 24 passed, 11 mismatched, score 68.6%). Two of the mismatches are the clusters below; both are status divergences, not message divergences.

Cause 1 - [mismatch] emr-containers-managed-endpoint-job-run-error-paths#task#2, ListManagedEndpoints:
  status: expected 200, got 400
  $.endpoints: missing (expected [], got None)

Cause 2 - [mismatch] emr-containers-managed-endpoint-job-run-error-paths#task#4, ListJobRuns:
  status: expected 200, got 400
  $.jobRuns: missing (expected [], got None)

The probe's virtual-cluster id is a per-run value and is not part of the requirement; the requirement is that a well-formed `virtualClusterId` that resolves to no cluster yields a 200 whose items member is present and empty.

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query operation=ListManagedEndpoints,ListJobRuns:
  ListManagedEndpoints: GET /virtualclusters/{virtualClusterId}/endpoints, http code 200; virtualClusterId -> ResourceIdString, required, httpLabel, length{min:1,max:64}, pattern ^[0-9a-z]+$; output {endpoints: Endpoints (list), nextToken: NextToken}; errors InternalServerException(500), ValidationException(400); paginated {inputToken: nextToken, outputToken: nextToken, items: endpoints, pageSize: maxResults}.
  ListJobRuns: GET /virtualclusters/{virtualClusterId}/jobruns, http code 200; same virtualClusterId constraints; output {jobRuns: JobRuns (list), nextToken: NextToken}; errors InternalServerException(500), ValidationException(400); paginated {items: jobRuns, ...}.
Neither operation binds any not-found error, so there is no modeled error the emulator's 400 could legitimately be carrying for an absent cluster.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay wire corpus simparity-wire-aws-v1, fixture emr-containers-managed-endpoint-job-run-error-paths.
3. Cause 1 (step task#2): call ListManagedEndpoints with a `virtualClusterId` that satisfies ResourceIdString (1-64 chars, ^[0-9a-z]+$) but names no existing virtual cluster. Expect HTTP 200 with `endpoints` present and equal to []; observed HTTP 400 with no `endpoints` member.
4. Cause 2 (step task#4): call ListJobRuns with the same unknown-but-valid `virtualClusterId`. Expect HTTP 200 with `jobRuns` present and equal to []; observed HTTP 400 with no `jobRuns` member.
5. Confirm the constraint path is unaffected: a `virtualClusterId` violating ResourceIdString (empty, >64 chars, or containing characters outside [0-9a-z]) must still be refused with HTTP 400 ValidationException.
6. Confirm the populated path is unaffected: create a virtual cluster, a managed endpoint and a job run, and check both list operations still return them.
7. Re-replay fixture emr-containers-managed-endpoint-job-run-error-paths steps task#2 and task#4; both must pass.
