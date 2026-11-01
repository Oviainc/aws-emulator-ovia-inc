Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
Issue-Key: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached|_run_count#29|KeyError|total += len(page["Runs"])
Issue-Commit: 2b09bad8439b0b0ac599f9d638bea059710099b4

# events__glue: Glue GetWorkflowRuns answers 200 with the Runs member absent for a workflow that has never run, instead of refusing with EntityNotFoundException

> _Note: this issue was identified against the code base at commit `2b09bad8439b0b0ac599f9d638bea059710099b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowRuns

## Description
One root cause. Cause count: 1.

Attribution note: the failing check's id leads with the two-service combo token `events__glue`, which equals no token returned by get_bootstrap_status (neither bootstrapped nor pending), so the per-service gate cannot place the composite literally. Both halves ARE stood up and answering in this replay (`service-com-amazonaws-cloudwatchevents-awsevents` / `service-com-amazonaws-eventbridge-awsevents` and `service-com-amazonaws-glue-awsglue` are all in the bootstrapped set), and the divergence described here is produced entirely by the glue crate's GetWorkflowRuns handler, so the issue is filed under the check's own token `events__glue` and the fix belongs in the glue service.

Related but NOT covering: open issue 20261026T234618Z-events-glue-with-glue-createtrigger-now-implemented-the.md carries this same check id and describes a DIFFERENT cause — that an EventBridge rule target naming a Glue workflow ARN never turns a delivery into a workflow run (the `glue:notifyEvent` ingress and the EVENT trigger's EventBatchingCondition accounting). That issue's recorded symptom was a bare 60 s harness timeout. The check no longer reaches that timeout: it now dies EARLIER, on the response SHAPE of GetWorkflowRuns, before any delivery is ever counted. Fixing the ingress would not fix the shape, and fixing the shape would not fix the ingress; the two are independent and this issue is only about the response shape.

=== Cause 1 of 1: GetWorkflowRuns on an existing workflow with zero runs returns HTTP 200 whose body omits `Runs` entirely, instead of refusing the read with EntityNotFoundException ===

AWS: Smithy com.amazonaws.glue#GetWorkflowRuns — input `Name` (NameString, @required, @length 1..255); output `Runs` (WorkflowRuns, @length min 1 max 1000, "A list of workflow run metadata objects") and `NextToken`; modeled errors EntityNotFoundException (HTTP 400, "A specified entity does not exist"), InvalidInputException (400), OperationTimeoutException (400), InternalServiceException (500). The operation is @paginated with items="Runs". Because the model gives `Runs` a minimum length of 1, there is no modeled representation of "this workflow exists but has no runs" in a 2xx body at all — and AWS does not invent one: for a workflow that has never run, GetWorkflowRuns is REFUSED with HTTP 400, error code `EntityNotFoundException`. The failing check states that contract in its own helper ("GetWorkflowRuns raises EntityNotFoundException until a workflow has run, so that code means zero runs") and treats that code — and only that code — as the encoding of zero runs. Message: fixed text is Glue's entity-not-found text for a workflow with no run history; its variable part is the workflow name taken from the request's `Name`. Hypothesis (neither the Smithy model nor the recording pins the exact string): AWS words it "No workflow runs were found." — the requirement this issue asserts is the 400 status and the `EntityNotFoundException` code; the emulator must not substitute an emulator-authored wording for whichever no-runs text AWS uses.

Emulator: the handler answers 200 and simply drops the member. services/service-com-amazonaws-glue-awsglue/src/ops/get_workflow_runs.rs builds its response as `runs: (!runs.is_empty()).then_some(runs)` (the comment there reasons "`Runs` is `length 1..1000`, so the model admits no empty list: a workflow no run has been started of ... report no `Runs` member at all rather than an empty one"). EntityNotFoundException is raised ONLY when the workflow name itself is unknown (`store.workflow_mut(&input.name)` miss). So a freshly created workflow yields a 200 body with neither `Runs` nor any error. The replay shows the consequence directly:

  [error] events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
  File "check_events__glue_event_trigger_fires_only_after_its_batch_size_is_reached.py", line 203, in _warm_up
    if _run_count(glue, workflow) >= 1:
  File "check_events__glue_event_trigger_fires_only_after_its_batch_size_is_reached.py", line 106, in _run_count
    total += len(page["Runs"])
  KeyError: 'Runs'

The check's `_run_count` catches `EntityNotFoundException` and maps it to 0; it has no branch for a 200 that omits `Runs`, because AWS never sends one. The emulator's answer is therefore neither of the two shapes AWS can produce.

Why: every client that reads a workflow's run history breaks on the very first read, and it breaks by CRASHING rather than by reporting "no runs". The canonical pattern — botocore's `get_paginator("get_workflow_runs")` plus `page["Runs"]`, or any SDK's generated accessor for a non-optional-in-practice list — raises a key/field error on an emulator response that is otherwise a success, so the caller cannot distinguish an empty history from a malformed service. A caller that defensively writes `page.get("Runs", [])` is silently told "zero runs" by a 200, which in AWS is impossible, and a caller that branches on EntityNotFoundException (the documented and only AWS encoding of zero runs, which the check relies on) never sees that branch taken. It also masks the state of any workflow-driven feature: the batching/ingress behaviour this check exists to grade is never reached, so the EventBatchingCondition contract stays entirely ungraded behind a response-shape bug.

Fix: the glue GetWorkflowRuns handler and its state read — services/service-com-amazonaws-glue-awsglue/src/ops/get_workflow_runs.rs. When the named workflow exists but its run history holds no run that this page would report (the zero-runs case, and consistently for a page past the end of the history), the operation must be refused with the modeled EntityNotFoundException at HTTP 400 carrying Glue's no-workflow-runs message for the requested `Name`, rather than returning 200 with `Runs` omitted. The sibling single-run reader and the workflow-not-found path already construct EntityNotFoundException in this file, so the refusal channel exists; what is missing is routing the empty-history case into it instead of into a 200.

Traces: events__glue.event-trigger-fires-only-after-its-batch-size-is-reached (dies in `_warm_up` -> `_run_count` with `KeyError: 'Runs'` on the paginated GetWorkflowRuns page for a just-created workflow).

## Evidence
Replay of simparity-behavioral-ga-prod-v1: events__glue 0/1 (0%), 1 failed.

[error] events__glue.event-trigger-fires-only-after-its-batch-size-is-reached
  Error: File "check_events__glue_event_trigger_fires_only_after_its_batch_size_is_reached.py", line 203, in _warm_up
    if _run_count(glue, workflow) >= 1:
  File "check_events__glue_event_trigger_fires_only_after_its_batch_size_is_reached.py", line 106, in _run_count
    total += len(page["Runs"])
  KeyError: 'Runs'

The check's helper, verbatim from the replayed source:

    def _run_count(glue, workflow: str) -> int:
        """How many runs this workflow has, through Glue's own reader. GetWorkflowRuns raises
        EntityNotFoundException until a workflow has run, so that code means zero runs."""
        try:
            total = 0
            for page in glue.get_paginator("get_workflow_runs").paginate(Name=workflow):
                total += len(page["Runs"])
            return total
        except ClientError as exc:
            if exc.response.get("Error", {}).get("Code") == "EntityNotFoundException":
                # The workflow itself is asserted to exist before this is ever called, so this can
                # only mean "no runs yet".
                return 0
            raise

Expected (AWS): glue:GetWorkflowRuns(Name=<a workflow created but never run>) -> HTTP 400 with
  Error.Code = "EntityNotFoundException"
  Error.Message = Glue's no-workflow-runs text, whose only variable part is the requested workflow Name
so a client's EntityNotFoundException branch is taken and "zero runs" is representable. The alternative AWS shape — a 200 carrying a `Runs` list of one or more WorkflowRun objects — is only produced once the workflow has at least one run; `Runs` is modeled @length min 1, so AWS never sends a 2xx with an empty or absent `Runs` for this operation.

Actual (emulator): HTTP 200 with a body containing neither `Runs` nor `NextToken`. No error is raised, so the client's EntityNotFoundException branch is unreachable and the direct member access raises KeyError: 'Runs'. The handler source makes this deliberate:

  services/service-com-amazonaws-glue-awsglue/src/ops/get_workflow_runs.rs:189-197  — EntityNotFoundException is raised only when `store.workflow_mut(&input.name)` finds no workflow of that NAME.
  services/service-com-amazonaws-glue-awsglue/src/ops/get_workflow_runs.rs:266-272  — `Ok(GetWorkflowRunsOutput { runs: (!runs.is_empty()).then_some(runs), next_token })`, with the comment "`Runs` is `length 1..1000`, so the model admits no empty list: a workflow no run has been started of, and a page past the end of its history, report no `Runs` member at all rather than an empty one."

Per-run values that are NOT part of the requirement: the generated workflow/trigger/job/role names, the account id, the region, and the harness's own poll budgets. The requirement is the status (400), the code (EntityNotFoundException), and that no 2xx answer for this operation ever omits `Runs`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1; check events__glue.event-trigger-fires-only-after-its-batch-size-is-reached fails with `KeyError: 'Runs'`.
2. By hand against the emulator endpoint, with no EventBridge wiring at all (the divergence needs only Glue):
   a. glue CreateWorkflow --name W  (any valid name; do not start any run)
   b. glue GetWorkflowRuns --name W
      -> AWS: HTTP 400, Error.Code "EntityNotFoundException", message = Glue's no-workflow-runs text naming W.
      -> Emulator: HTTP 200 with a body that contains no `Runs` member (and no `NextToken`).
   c. In Python: `list(boto3.client('glue', endpoint_url=<emulator>).get_paginator('get_workflow_runs').paginate(Name='W'))[0]['Runs']`
      -> AWS: raises botocore ClientError EntityNotFoundException (the documented encoding of zero runs).
      -> Emulator: raises `KeyError: 'Runs'` on a 200 response.
3. glue GetWorkflowRuns --name <a name no workflow exists under> -> both AWS and the emulator answer 400 EntityNotFoundException, confirming the refusal channel is wired and that only the existing-workflow/zero-runs case is misrouted into a 200.
4. After the fix, re-replay events__glue.event-trigger-fires-only-after-its-batch-size-is-reached: it must get past `_warm_up`/`_run_count` (note the separately-filed EventBridge->Glue ingress cause, open issue 20261026T234618Z-events-glue-with-glue-createtrigger-now-implemented-the.md, will then be the next thing it hits; that one is not re-filed here).
