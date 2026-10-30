Issue-Source: trace-sweep-divergence
Issue-Trace: lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete
Issue-Shape-Id: com.amazonaws.lambdacore#LambdaCoreApiService
Issue-Key: lambda-core|UpdateNetworkConnector|2xx->2xx|$.LastUpdateStatusReason
Issue-Key: lambda-core|GetNetworkConnector|2xx->2xx|$.LastUpdateStatusReason
Issue-Key: lambda-core|GetNetworkConnector|2xx->2xx|$.StateReason
Issue-Commit: 87f40be06f8bd7b2972565eaaefcbb318e4a8aaf

# lambda-core UpdateNetworkConnector never reports LastUpdateStatusReason and leaves StateReason at its creation text (2 causes)

> _Note: this issue was identified against the code base at commit `87f40be06f8bd7b2972565eaaefcbb318e4a8aaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateNetworkConnector

## Description
Two net-new root causes: an update of a network connector never populates the update-status narrative members, and the connector's StateReason is never re-stated when an update is initiated. Both are successful responses (202/200), so no error path is involved - the projections are simply incomplete.

Cause 1 of 2 - UpdateNetworkConnector's 202 response omits LastUpdateStatusReason.
AWS: Smithy UpdateNetworkConnector (PUT /2026-04-04/network-connectors/{Identifier}, success code 202) outputs LastUpdateStatus -> NetworkConnectorLastUpdateStatus (enum [Successful, Failed, InProgress]) alongside LastUpdateStatusReason -> NetworkConnectorLastUpdateStatusReason (string), the human-readable companion to that status. Recorded behavior: the accepted update answers 202 carrying LastUpdateStatusReason with the fixed text 'Update in progress' for an update that has been accepted and not yet settled - the text is a status narrative, with no request- or state-derived variable part.
Emulator: answers 202 but omits LastUpdateStatusReason entirely (the member is absent from the body).
Why: a client that polls the update by reading LastUpdateStatus/LastUpdateStatusReason has no reason text at all, so any display or log line built from it is empty and code that branches on the reason string sees None where AWS always supplies a value for an in-flight update.
Fix: UpdateNetworkConnector's handler/serializer must set the connector's last-update status reason when it transitions the connector into an in-progress update, and include the member in the response projection.
Traces: lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete#task#10.

Cause 2 of 2 - after an update, GetNetworkConnector still reports the creation-time StateReason and omits LastUpdateStatusReason.
AWS: GetNetworkConnector (GET /2026-04-04/network-connectors/{Identifier}, 200) outputs both StateReason (String, the narrative for State/StateReasonCode) and LastUpdateStatusReason (the narrative for LastUpdateStatus). Recorded behavior: once an update has been accepted, a read of the connector reports StateReason with the fixed text 'Update initiated' and, once the update has settled, LastUpdateStatusReason with the fixed text 'Update was successful'; both are status narratives with no variable parts. StateReason therefore tracks the connector's current lifecycle event rather than remaining at its creation value.
Emulator: returns 200 with StateReason still set to its creation text ('Initial creation') and LastUpdateStatusReason absent, both before and after the update settles - the same divergence recurs on a later read in the same fixture (task#16), so it is the stored projection, not a timing artifact.
Why: the update is invisible in the connector's own description: a client reading the resource cannot tell an untouched connector from one that has just been updated, and cannot learn whether the last update succeeded, so convergence waits and drift detectors built on these members never observe the update.
Fix: the connector's state model must record a state reason on the update transition and a last-update status reason on its terminal outcome, and GetNetworkConnector's projection must emit both members.
Traces: lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete#task#11 (and the same cluster's task#16).

## Evidence
Replay of corpus simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false).

Cause 1 - lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete#task#10, UpdateNetworkConnector: status 202 on both sides; $.LastUpdateStatusReason missing (expected 'Update in progress', got None). divergence_key service=lambda-core|UpdateNetworkConnector|status=202->202|keys=$.LastUpdateStatusReason.

Cause 2 - lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete#task#11, GetNetworkConnector: status 200 on both sides; $.LastUpdateStatusReason missing (expected 'Update was successful', got None); $.StateReason value_mismatch (expected 'Update initiated', got 'Initial creation'). affected_count=2 - the same divergence repeats at step task#16 of the same fixture. divergence_key service=lambda-core|GetNetworkConnector|status=200->200|keys=$.LastUpdateStatusReason,$.StateReason.

Model references (services/service-com-amazonaws-lambdacore-lambdacoreapiservice/smithy-model/com-amazonaws-lambdacore-lambdacoreapiservice.json): UpdateNetworkConnector output members LastUpdateStatus (NetworkConnectorLastUpdateStatus enum [Successful, Failed, InProgress]) and LastUpdateStatusReason (NetworkConnectorLastUpdateStatusReason, string), http code 202; GetNetworkConnector output members StateReason (String), StateReasonCode (NetworkConnectorStateReasonCode), LastUpdateStatus, LastUpdateStatusReason, LastUpdateStatusReasonCode, http code 200.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus() to replay simparity-wire-aws-v1.
2. Cause 1: fixture lambdacore-list-network-connector-versions-after-update-and-version-scoped-delete, step task#10 - PUT /2026-04-04/network-connectors/{Identifier} with a changed Configuration; inspect the 202 body for LastUpdateStatusReason (absent; AWS sends 'Update in progress').
3. Cause 2: same fixture, step task#11 (and again at task#16) - GET /2026-04-04/network-connectors/{Identifier} after that update; inspect StateReason (still 'Initial creation'; AWS reports 'Update initiated') and LastUpdateStatusReason (absent; AWS reports 'Update was successful' once the update settles).
