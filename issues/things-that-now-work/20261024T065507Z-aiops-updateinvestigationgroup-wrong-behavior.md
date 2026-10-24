Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# UpdateInvestigationGroup refuses the investigation group NAME as identifier and answers 404 instead of updating

_Filed: 2026-10-24T06:55:07Z_
_Operation: UpdateInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateInvestigationGroup's `identifier` is documented as "Specify either the name or the ARN of the investigation group that you want to modify", and the modelled identifier pattern admits a bare label.

Expected: PATCH /investigationGroups/my-group with e.g. {"isCloudTrailEventHistoryEnabled": false} applies the change to the group named `my-group` in the signing Region and returns HTTP 200; a following GetInvestigationGroup reports the new value and an advanced lastModifiedAt.

Actual: the shared ARN-only resolver returns nothing for the name form, so the update is answered HTTP 404 `ResourceNotFoundException` and no stored member changes. The same resolver backs PutInvestigationGroupPolicy, GetInvestigationGroupPolicy and DeleteInvestigationGroupPolicy, so configuring a group by name fails there too.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.aiops#UpdateInvestigationGroupRequest", member="identifier")` -> "Specify either the name or the ARN of the investigation group that you want to modify."; `InvestigationGroupIdentifier` = `^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group/[A-Za-z0-9]{16})$`.

Handler/state: services/service-com-amazonaws-aiops-aiops/src/ops/update_investigation_group.rs line ~217 `store.group_mut(&input.identifier, &region)` -> services/service-com-amazonaws-aiops-aiops/src/state.rs `group_mut()` -> `resolve_group_id()`, which requires `group_id_from_arn(identifier)` and an exact `group.arn == identifier` match.

Test coverage: tests/all/investigation_group_singleton.rs::only_the_arn_resolves_a_group asserts a 404 for UpdateInvestigationGroup addressed by the group's own name, so the divergent behaviour is pinned by an existing test rather than merely unasserted.

## Steps to Reproduce

1. POST /investigationGroups {"name":"my-group","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} -> 201
2. PATCH /investigationGroups/my-group {"isCloudTrailEventHistoryEnabled": false} -> emulator answers 404 ResourceNotFoundException; real AWS answers 200 and applies the change
3. GET /investigationGroups/<url-encoded ARN> -> isCloudTrailEventHistoryEnabled is still true, confirming nothing was applied
4. POST /investigationGroups/my-group/policy {"policy":"{\"Version\":\"2012-10-17\"}"} -> also 404, showing the policy operations share the same ARN-only resolution
