Issue-Source: trace-sweep-divergence
Issue-Trace: transfer-workflow-definition-and-execution-introspection
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: transfer-workflow-definition-and-execution-introspection
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|ListWorkflows|2xx->2xx|$.Workflows[*]
Issue-Commit: 16c1636731f072135c908023c0b09afa04d105ac

# Transfer ListWorkflows reports an empty Workflows list while the account holds a workflow

> _Note: this issue was identified against the code base at commit `16c1636731f072135c908023c0b09afa04d105ac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListWorkflows

## Description
Cause 1 of 1: ListWorkflows does not report a workflow the account holds, so the workflow plane is invisible to a discovery call even though the single-resource reads of the same workflow answer.

AWS: ListWorkflows "Lists all workflows associated with your Amazon Web Services account for your current region" and its output member `Workflows` (ListedWorkflows) is @required, each entry carrying `Arn`, `WorkflowId` and `Description` (Smithy: ListWorkflows input MaxResults range 1..1000 / NextToken length 1..6144; output Workflows -> ListedWorkflows required, NextToken; paginated inputToken=NextToken outputToken=NextToken items=Workflows pageSize=MaxResults; errors InvalidNextTokenException, InvalidRequestException, InternalServiceError, ServiceUnavailableException). A workflow minted by CreateWorkflow (output WorkflowId, pattern ^w-([a-z0-9]{17})$) is listable immediately — the recorded step, taken after the fixture created a workflow whose `Description` was the one it submitted, returns 200 with that workflow as the single entry (`Arn` = arn:aws:transfer:<region>:<account>:workflow/<workflowId>, `WorkflowId` = the id CreateWorkflow returned, `Description` = the description submitted at create time). The identifier, ARN and description are values taken from the request and the service's state, not fixed text.

Emulator: the same step answers 200 but with no entry for that workflow — the replay reports `$.Workflows[0]: missing (expected {'Arn': 'arn:aws:transfer:...:workflow/{{workflow_id}}', 'Description': 'probe workflow', 'WorkflowId': '{{workflow_id}}'}, got None)`, i.e. the required `Workflows` list comes back empty/short while the create that minted the id succeeded (no other step of this fixture diverged, so the create and the single-workflow reads around it matched the recording — only the listing omits the record).

Why: a client that discovers workflows before attaching one to a server's WorkflowDetails, or that pages ListWorkflows to reconcile state, concludes the account has no workflows at all. Create-then-list and every paginator built on ListWorkflows (NextToken/MaxResults) return nothing, so provisioning flows that look a workflow up by listing it dead-end, and a `--query` or `jq` read of `Workflows[0]` errors out instead of returning the workflow just created.

Fix: the ListWorkflows handler and the workflow store it reads — services/service-com-amazonaws-transfer-transferservice/src/ops/list_workflows.rs projects `guard.workflows` through state::page_bounds/page_limit, and services/service-com-amazonaws-transfer-transferservice/src/ops/create_workflow.rs inserts into the same `workflows` BTreeMap (state.rs field `pub workflows: BTreeMap<String, Workflow>`), so the created record is not reaching the map the listing walks at the time the listing runs. Hypothesis: the listing observes a workflow store that the create's write is not visible in (or that an earlier step in the same session cleared), since the per-page projection itself is well-formed; neither the model nor the recording states the mechanism.

Traces: transfer-workflow-definition-and-execution-introspection#task#4 (ListWorkflows, expected 200 with one entry, got 200 with the entry missing).

## Evidence
Fixture transfer-workflow-definition-and-execution-introspection, step task#4, operation ListWorkflows.
Expected (recorded AWS): status 200, $.Workflows[0] = {'Arn': 'arn:aws:transfer:us-west-2:000000000000:workflow/{{workflow_id}}', 'Description': 'probe workflow', 'WorkflowId': '{{workflow_id}}'} — the ARN/id are the values the fixture captured from CreateWorkflow and the description is the one the create submitted.
Actual (emulator): status 200, $.Workflows[0] missing (got None) — the required Workflows member carries no entry for the workflow the account holds.
No other step of this fixture reported a divergence, so the workflow was created successfully and only the listing fails to report it.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Equivalent by hand: `aws transfer create-workflow --description 'probe workflow' --steps <one valid step>` -> note the returned WorkflowId.
3. `aws transfer list-workflows` against the same endpoint.
4. AWS returns Workflows containing that WorkflowId with its Arn and Description; the emulator returns 200 with an empty Workflows list (fixture transfer-workflow-definition-and-execution-introspection, step task#4).
