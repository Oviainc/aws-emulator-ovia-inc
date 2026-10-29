Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amp.workspace-alias-update-round-trip
Issue-Shape-Id: com.amazonaws.amp#AmazonPrometheusService
Issue-Key: amp.workspace-alias-update-round-trip|run#14|AssertionError|assert described["status"]["statusCode"] == "ACTIVE"
Issue-Commit: df009dd79b3a670d3c0c53c12bb5e37a115b862c

# UpdateWorkspaceAlias publishes the new alias while the workspace still reports UPDATING, so the rename's completion signal and the workspace status disagree

> _Note: this issue was identified against the code base at commit `df009dd79b3a670d3c0c53c12bb5e37a115b862c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateWorkspaceAlias

## Description
One cause (Cause 1 of 1): UpdateWorkspaceAlias writes the new alias and the transient UPDATING status in the SAME state write, so DescribeWorkspace reports the NEW alias together with status.statusCode == "UPDATING". That combined state is not reachable on real AWS, and a client that treats the visible new alias as the rename's completion signal then finds the workspace not ACTIVE.

AWS: Smithy model (services/service-com-amazonaws-amp-amazonprometheusservice/smithy-model/com-amazonaws-amp-amazonprometheusservice.json): UpdateWorkspaceAlias POST /workspaces/{workspaceId}/alias, @idempotent, modelled success 204, output Unit; input members workspaceId (@httpLabel, @required, WorkspaceId len 1..64, pattern [0-9A-Za-z][-.0-9A-Z_a-z]*), alias (WorkspaceAlias len 1..100, documented "Amazon Managed Service for Prometheus will automatically strip any blank spaces from the beginning and end of the alias"), clientToken (@idempotencyToken). DescribeWorkspace GET /workspaces/{workspaceId} returns WorkspaceDescription with @required status (WorkspaceStatus.statusCode, enum WorkspaceStatusCode: CREATING/ACTIVE/UPDATING/DELETING). The model documents WorkspaceStatusCode$UPDATING as "Workspace is being updated. Updates are allowed only when status is ACTIVE." but it does NOT say an alias rename enters that status, and it does not state any ordering between the alias becoming visible and the status settling. Recorded behavior (the authority here): against AWS, as soon as DescribeWorkspace reports the new alias, the same DescribeWorkspace reports status.statusCode == "ACTIVE" and an unchanged workspaceId/arn — i.e. on AWS the new alias is never observable while the workspace is non-ACTIVE. Hypothesis: AMP applies an alias rename as metadata on the live workspace, so the workspace stays ACTIVE for the whole rename (alternatively it publishes the new alias only once the update has settled); either way the pair (new alias, UPDATING) never appears.

Emulator: services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/update_workspace_alias.rs sets workspace.alias = normalize_alias(input.alias) and, in the same block, workspace.status_code = STATUS_UPDATING with transition_due = now + crate::state::TRANSITION_DELAY (500 ms, services/.../src/state.rs). The clock settles the status back to ACTIVE only later, in ComAmazonawsAmpAmazonprometheusserviceState::materialize_due. So immediately after the 204, DescribeWorkspace answers 200 with workspace.alias already == the new alias and workspace.status.statusCode == "UPDATING". The replayed check polls DescribeWorkspace until alias == the new alias (which the emulator satisfies on the FIRST probe, with no wait), then asserts the describe's status.statusCode == "ACTIVE" and gets "UPDATING": AssertionError at check_amp_workspace_alias_update_round_trip.py line 42. workspaceId and arn are reported correctly; only the status diverges.

Why: a client relying on AWS's behavior uses the visible new alias as the signal that the rename finished and then proceeds to the next control-plane call. On this emulator the workspace is still UPDATING at that moment, so the follow-on call is refused: the same handler returns ConflictException (HTTP 409) with message "The workspace is UPDATING; updates are allowed only when status is ACTIVE" (fixed text "The workspace is " / "; updates are allowed only when status is " with the workspace's current status code and the literal ACTIVE substituted from state), and the other status-gated workspace operations refuse the same way. A client that instead waits for ACTIVE burns an extra poll cycle that AWS never requires. Readiness logic calibrated on AWS therefore mis-sequences against the emulator, and any assertion that the alias and the status are mutually consistent fails.

Fix: the UpdateWorkspaceAlias handler's state transition — services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/update_workspace_alias.rs (the block that assigns workspace.alias, workspace.status_code and workspace.transition_due) together with the workspace branch of ComAmazonawsAmpAmazonprometheusserviceState::materialize_due in services/service-com-amazonaws-amp-amazonprometheusservice/src/state.rs. The two observable facts (new alias visible, status ACTIVE) must become visible as one step, so no caller can ever read the new alias on a workspace that is not ACTIVE; the idempotency bookkeeping on update_token must keep working for a replay of the same clientToken.

Traces: amp.workspace-alias-update-round-trip (the only failing check; no existing open amp issue covers the alias/status ordering — the open amp issues concern prometheusEndpoint host, TagResource error code/validation, ListWorkspaces region scoping, UpdateQueryLoggingConfiguration client-token replay, UpdateScraper validation and the anomaly-detector handlers).

## Evidence
Check amp.workspace-alias-update-round-trip, verdict mismatch.

Expected (recorded AWS): after CreateWorkspace -> ACTIVE, UpdateWorkspaceAlias(workspaceId, alias=<new>) answers 204; DescribeWorkspace then reports workspace.alias == <new>, workspace.workspaceId unchanged, workspace.arn unchanged, and workspace.status.statusCode == "ACTIVE".

Actual (emulator): DescribeWorkspace reports workspace.alias == <new> on the first probe after the 204, with workspace.status.statusCode == "UPDATING". workspaceId and arn match. Failure:
  File "check_amp_workspace_alias_update_round_trip.py", line 42, in run
    assert described["status"]["statusCode"] == "ACTIVE"
  AssertionError

Source of the divergence: services/service-com-amazonaws-amp-amazonprometheusservice/src/ops/update_workspace_alias.rs lines 233-248 assign workspace.alias and then workspace.status_code = STATUS_UPDATING / transition_due = now + TRANSITION_DELAY in one write; TRANSITION_DELAY = 500 ms (services/service-com-amazonaws-amp-amazonprometheusservice/src/state.rs line 185); the status returns to ACTIVE only in ComAmazonawsAmpAmazonprometheusserviceState::materialize_due (state.rs lines ~1100-1109).

Status/error contract observed while the workspace is wrongly non-ACTIVE: a second UpdateWorkspaceAlias in that window answers HTTP 409 ConflictException "The workspace is UPDATING; updates are allowed only when status is ACTIVE" (update_workspace_alias.rs lines 218-231), where "UPDATING" is the stored status and "ACTIVE" is the required one.

## Steps to Reproduce
1. CreateWorkspace(alias=<any>, tags={...}) -> note workspaceId.
2. Poll DescribeWorkspace(workspaceId) until workspace.status.statusCode == "ACTIVE"; record workspace.arn.
3. UpdateWorkspaceAlias(workspaceId=<id>, alias=<a different alias>) -> 204.
4. Immediately call DescribeWorkspace(workspaceId) in a loop until workspace.alias == the new alias (the emulator satisfies this on the first call).
5. Inspect that same response: AWS reports status.statusCode == "ACTIVE"; the emulator reports "UPDATING" (workspaceId and arn are unchanged in both).
6. Within the same window, issue another UpdateWorkspaceAlias for that workspace: the emulator answers 409 ConflictException "The workspace is UPDATING; updates are allowed only when status is ACTIVE".
