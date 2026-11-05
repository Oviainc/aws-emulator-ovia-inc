Issue-Source: trace-sweep-divergence
Issue-Trace: nova-act-session-and-act-workflow
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: nova-act-session-and-act-workflow
Issue-Shape-Id: com.amazonaws.novaact#AmazonNovaAgentsDataPlane
Issue-Key: nova-act|ListActs|2xx->2xx|$.actSummaries[*]
Issue-Commit: 9de1430d35a30df63dfe38bccf7c2cf4feacacf3

# nova-act ListActs reports a constant act status instead of the stored act state

> _Note: this issue was identified against the code base at commit `9de1430d35a30df63dfe38bccf7c2cf4feacacf3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListActs

## Description
One net-new divergence in the nova-act emulator. 1 cause must be closed.

Cause 1 of 1: ListActs serves a fixed act status in every ActSummary rather than the act's stored execution state (fixture nova-act-session-and-act-workflow, step task#8).

AWS: Smithy ListActs - HTTP POST /workflow-definitions/{workflowDefinitionName}/acts, success code 200, readonly, paginated over actSummaries; output ListActsResponse.actSummaries is required, target ActSummaries of ActSummary. ActSummary.status is REQUIRED and targets the enum ActStatus (RUNNING | PENDING_CLIENT_ACTION | PENDING_HUMAN_ACTION | SUCCEEDED | FAILED | TIMED_OUT), documented as "The current execution status of the act"; ActSummary.startedAt is a required DateTimestamp (@timestampFormat date-time) documented as "The timestamp when the act started execution", and endedAt is the optional completion timestamp. Because status is documented as the act's CURRENT execution status, a read-only listing must project the state the act is actually in, and it must agree with what the act-creating/act-advancing operations reported for that same actId. The model does not state which enum member a given act holds at a given moment, so: recorded behavior only - the recording's ListActs for the session created earlier in the same fixture returns 200 with a single summary whose status is PENDING_CLIENT_ACTION, i.e. the same state CreateAct reported for that actId, namely an act waiting for the caller to run a tool and report its result.

Emulator: HTTP status matches (200) and the identifiers match (workflowRunId, sessionId and actId are the ones created earlier in the fixture), but $.actSummaries[0].status is the constant 'RUNNING' where the recording reports 'PENDING_CLIENT_ACTION'. The replay reports the whole element as diverging:
  $.actSummaries[0]: expected status 'PENDING_CLIENT_ACTION', got 'RUNNING'.
The element's startedAt also differs, but that is a per-run clock value and is NOT part of this finding; the requirement is only that startedAt be a date-time for the instant the act started, which the emulator does emit.

Why: a client enumerates acts to find out which ones are waiting on it. PENDING_CLIENT_ACTION is its instruction to execute the requested tool and submit the result; PENDING_HUMAN_ACTION, SUCCEEDED, FAILED and TIMED_OUT are its signals to escalate, finish or give up. Told that every listed act is RUNNING, the client concludes the service is still working and keeps polling a listing that will never change, so no act is ever advanced and the session stalls until it times out. It also makes ListActs contradict the per-act views of the same act inside one response cycle, so reconciliation logic built on the listing (resume-after-restart, dashboards, "which acts need me?" queries) is wrong for every act that is not actually running.

Fix: the ListActs handler in the nova-act service crate - build each ActSummary by projecting the stored act record (status, startedAt, endedAt, traceLocation) out of act state instead of emitting a literal status, so the listing's status is the same value the single-act views report for that actId.

Related open issues cover DIFFERENT causes and do NOT cover this one: 20261104T064305Z-nova-act-returns-static-placeholder-state-instead-of.md owns the CreateAct, ListModels and GetWorkflowRun placeholder-state clusters (its replay key does not reach ListActs), and 20261023T043407Z-nova-act-createworkflowdefinition-skips-precondition-checks-and-returns.md owns the CreateWorkflowDefinition precondition divergences in this same replay. The ListActs cluster is reported with no owner, so it is filed here; the same underlying state derivation may be where it is fixed.

Traces: nova-act-session-and-act-workflow#task#8 (ListActs, expected 200 / actual 200).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health answered 200 and the data listener accepts): 51 steps total, 32 passed, 19 mismatched, 0 errored, 0 blocked, score 62.7%, 6 divergence clusters.

Cause 1 - ListActs, expected 200 / actual 200, cluster key service=nova-act|ListActs|status=200->200|keys=$.actSummaries[0], affected: nova-act-session-and-act-workflow#task#8 (affected_count 1), stub_path empty, owned_by empty:
  $.actSummaries[0]: missing
    expected {'actId': '<actId created earlier in the fixture>', 'sessionId': '<sessionId>', 'startedAt': '<RFC3339 start instant>', 'status': 'PENDING_CLIENT_ACTION', 'workflowRunId': '<workflowRunId>'}
    got      {'workflowRunId': '<same workflowRunId>', 'sessionId': '<same sessionId>', 'actId': '<same actId>', 'status': 'RUNNING', 'startedAt': '<emulator clock instant>'}

The only requirement-bearing difference is $.actSummaries[0].status: 'PENDING_CLIENT_ACTION' expected, 'RUNNING' served. The identifiers match. The startedAt difference is a per-run clock value and is excluded from this finding.

Model references used: ListActs (http POST /workflow-definitions/{workflowDefinitionName}/acts code 200, readonly, paginated items=actSummaries; ListActsResponse.actSummaries required), ActSummary (status required -> ActStatus, doc "The current execution status of the act"; startedAt required DateTimestamp @timestampFormat date-time; endedAt optional DateTimestamp), ActStatus (enum RUNNING | PENDING_CLIENT_ACTION | PENDING_HUMAN_ACTION | SUCCEEDED | FAILED | TIMED_OUT).

## Steps to Reproduce
1. git_clone the project and boot the emulator built from that checkout.
2. replay_wire_corpus() against trace set simparity-wire-aws-v1.
3. Fixture nova-act-session-and-act-workflow, step task#8: run the fixture's earlier steps so a workflow definition, workflow run, session and act exist and the act was created in a state that waits on the client, then POST /workflow-definitions/{workflowDefinitionName}/acts?workflowRunId=<runId>&sessionId=<sessionId>.
4. Observe the 200 body: $.actSummaries[0].status is 'RUNNING' where the recording reports 'PENDING_CLIENT_ACTION' for the same actId that CreateAct returned earlier in the fixture.
5. After the fix, re-replay nova-act-session-and-act-workflow#task#8; the listing's status must equal the stored act state for that actId and the step must pass.
