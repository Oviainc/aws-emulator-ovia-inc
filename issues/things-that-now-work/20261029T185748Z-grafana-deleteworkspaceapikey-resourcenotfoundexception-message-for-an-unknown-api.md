Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: grafana.workspace-api-key-lifecycle-round-trip
Issue-Shape-Id: com.amazonaws.grafana#AWSGrafanaControlPlane
Issue-Key: grafana.workspace-api-key-lifecycle-round-trip|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# grafana DeleteWorkspaceApiKey: ResourceNotFoundException message for an unknown API key name does not follow AWS's message rule

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWorkspaceApiKey

## Description
One net-new root cause in the `grafana` service (the only failing behavioral check not already filed/owned: the other five grafana failures are covered by the unimplemented-`DeleteWorkspaceServiceAccount` issue filed for me and by the open issue 20261022T092712Z-grafana-workspace-control-plane-6-behavioral-divergences-delete.md).

=== Cause 1 of 1: the api-key-not-found message names the workspace and omits the terminating period, instead of AWS's `API key <keyName> not found.` ===

AWS: Smithy `DeleteWorkspaceApiKey` is `DELETE /workspaces/{workspaceId}/apikeys/{keyName}`, success status 200, input members `workspaceId` (@httpLabel, @required, pattern ^g-[0-9a-f]{10}$) and `keyName` (@httpLabel, @required, @length 1..100), and it models `ResourceNotFoundException` (HTTP 404, documented "The request references a resource that does not exist"). For a second delete of a key that has already been deleted (so the workspace exists and is ACTIVE, but holds no key of that name) the model fixes the status (404) and the code (`ResourceNotFoundException`); the message rule is recorded behavior only: AWS composes it as the fixed text `API key ` + the `keyName` taken verbatim from the request + the fixed text ` not found.` — i.e. the only variable part is the requested key name, the workspace id is NOT named, and the sentence ends in a period. (In the replayed run the key name was a per-run generated name, so that name is a value filling the template, not part of it.)

Emulator: status 404 and code `ResourceNotFoundException` match, but the message is `API key <keyName> not found in workspace <workspaceId>` — it appends the fixed text ` not found in workspace ` plus the request's workspace id and emits no trailing period. Observed in the replay: expected `'API key sim2real-behav-key-1164668d0968 not found.'`, got `'API key sim2real-behav-key-1164668d0968 not found in workspace g-0000000001'`.

Why: the message is part of the rejection contract for this operation. A client that surfaces, logs or asserts on the not-found text for a repeated/idempotent API-key delete sees a differently-shaped sentence than AWS produces for the same request, including a workspace id AWS never puts there; anything matching AWS's exact sentence (or stripping its trailing period) fails against the emulator even though the code and status are right.

Fix: the shared `ResourceNotFoundException` builder for API keys in this service's state — it must compose the message from the requested key name alone, in AWS's form `API key <keyName> not found.`, while keeping 404/`ResourceNotFoundException` and the resource_id/resource_type it already sets. Both callers of that builder (the repeated delete and any other api-key read path) then report the one AWS sentence. Code: services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/state.rs:982-988 (`api_key_not_found`), used at services/service-com-amazonaws-grafana-awsgrafanacontrolplane/src/ops/delete_workspace_api_key.rs:177-182.

Traces: grafana.workspace-api-key-lifecycle-round-trip.

## Evidence
Behavioral suite: total 13/19 passed; grafana 13/19 (68%), 6 failed. Five of the six failures are already filed (1 unimplemented-operation gap for DeleteWorkspaceServiceAccount, 4 OWNED by 20261022T092712Z-grafana-workspace-control-plane-6-behavioral-divergences-delete.md) and are excluded here. The remaining failure:

grafana.workspace-api-key-lifecycle-round-trip [mismatch]
  The check's positive controls all pass: CreateWorkspaceApiKey issues a secret that authenticates against the workspace's own Grafana HTTP API (`GET /api/org` returns 200 with id+name), DeleteWorkspaceApiKey succeeds, and the deleted secret is subsequently refused (401/403) at the workspace endpoint.
  The divergence is the SECOND DeleteWorkspaceApiKey for the now-gone key. Error code matches (ResourceNotFoundException, HTTP 404).
  Expected message rule: 'API key <keyName> not found.' (fixed text 'API key ' + request keyName + ' not found.'; example from the run: 'API key sim2real-behav-key-1164668d0968 not found.').
  Actual: 'API key sim2real-behav-key-1164668d0968 not found in workspace g-0000000001' — extra ' in workspace <workspaceId>' clause, no trailing period.
  Assertion: check_grafana_workspace_api_key_lifecycle_round_trip.py line 58: `assert message == expected`.

## Steps to Reproduce
Boot the emulator and replay grafana.workspace-api-key-lifecycle-round-trip, or by hand:
1. iam.CreateRole with a trust policy for grafana.amazonaws.com; grafana.CreateWorkspace(accountAccessType=CURRENT_ACCOUNT, permissionType=SERVICE_MANAGED, authenticationProviders=['SAML'], workspaceRoleArn=<role arn>, workspaceName=<name>); poll DescribeWorkspace until status=ACTIVE and take `endpoint`.
2. grafana.CreateWorkspaceApiKey(workspaceId=<id>, keyName=<key>, keyRole='ADMIN', secondsToLive=3600) -> 200 with a usable `key`.
3. grafana.DeleteWorkspaceApiKey(workspaceId=<id>, keyName=<key>) -> 200 (succeeds; the secret stops authenticating at the workspace endpoint).
4. grafana.DeleteWorkspaceApiKey(workspaceId=<id>, keyName=<key>) again -> 404 ResourceNotFoundException. Compare Error.Message against AWS's rule 'API key <key> not found.'; the emulator answers 'API key <key> not found in workspace <id>'.
