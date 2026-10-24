Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# DeleteInvestigationGroup refuses the investigation group NAME as identifier and answers 404 instead of deleting

_Filed: 2026-10-24T06:55:07Z_
_Operation: DeleteInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

DeleteInvestigationGroup's `identifier` is documented as "Specify either the name or the ARN of the investigation group that you want to delete", and `InvestigationGroupIdentifier` admits a bare `[\-_A-Za-z0-9]{1,512}` label beside the ARN.

Expected: DELETE /investigationGroups/my-group, where a group named `my-group` exists in the signing Region, deletes that group and returns HTTP 200; a later GetInvestigationGroup on its ARN then returns 404.

Actual: the shared resolver only accepts the exact stored ARN, so the name form resolves to nothing and the delete is answered HTTP 404 `ResourceNotFoundException` while the group remains in the store — a caller following the documented usage cannot delete the group, and because the service enforces one group per Region, the next CreateInvestigationGroup is then refused with 402 ServiceQuotaExceededException, wedging the lifecycle.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.aiops#DeleteInvestigationGroupRequest", member="identifier")` -> "Specify either the name or the ARN of the investigation group that you want to delete."; `InvestigationGroupIdentifier` pattern allows the bare-name alternative.

Handler/state: services/service-com-amazonaws-aiops-aiops/src/ops/delete_investigation_group.rs line ~179 `store.remove_group(&input.identifier, &region)` -> services/service-com-amazonaws-aiops-aiops/src/state.rs `remove_group()` -> `resolve_group_id()`, which starts with `group_id_from_arn(identifier)?` and so returns `None` for any non-ARN identifier.

Test coverage: tests/all/investigation_group_singleton.rs::only_the_arn_resolves_a_group asserts 404 for DeleteInvestigationGroup addressed by "the group's own name" — i.e. the current (divergent) behaviour is pinned by a test that must change with the fix.

## Steps to Reproduce

1. POST /investigationGroups {"name":"my-group","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} -> 201
2. DELETE /investigationGroups/my-group -> emulator answers 404 ResourceNotFoundException; real AWS deletes the group and answers 200
3. GET /investigationGroups -> the group is still listed, so nothing was deleted
4. POST /investigationGroups {"name":"my-group-2",...} -> 402 ServiceQuotaExceededException, because the undeleted group still occupies the Region's single slot
