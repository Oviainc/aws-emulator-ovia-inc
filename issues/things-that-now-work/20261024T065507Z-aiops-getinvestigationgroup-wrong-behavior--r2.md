Issue-Severity: HIGH
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b

# GetInvestigationGroup refuses the investigation group NAME as identifier and answers 404 for a group that exists

_Filed: 2026-10-24T06:55:07Z_
_Operation: GetInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3db28a81d798fa5ec4f024b9e26d5db0ef48cf2b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The `identifier` member of GetInvestigationGroup is documented as "Specify either the name or the ARN of the investigation group that you want to view", and the modelled `InvestigationGroupIdentifier` pattern explicitly admits a bare label (`^(?:[\-_A-Za-z0-9]{1,512}|arn:...:investigation-group/[A-Za-z0-9]{16})$`) beside the ARN form. So a caller may address a group by its name (e.g. `aws aiops get-investigation-group --identifier my-group`).

Expected: GET /investigationGroups/my-group, where an investigation group named `my-group` exists in the signing Region, returns HTTP 200 with that group's body (arn, name, roleArn, retentionInDays, createdAt, ...).

Actual: the emulator resolves an identifier from the ARN form and from nothing else, so the name form (and the bare-id form) resolves to no group and the request is answered HTTP 404 `ResourceNotFoundException` (`"InvestigationGroup does not exist."`) even though the group exists and is listed by ListInvestigationGroups. The ARN-only rule lives in one shared resolver, so the same divergence is observable on UpdateInvestigationGroup, DeleteInvestigationGroup, GetInvestigationGroupPolicy, PutInvestigationGroupPolicy and DeleteInvestigationGroupPolicy — any SDK/CLI/IaC flow that keeps the group's name rather than its ARN (the documented usage) is broken end to end.

## Evidence

Spec: `smithy_query(query="doc", shape="com.amazonaws.aiops#GetInvestigationGroupRequest", member="identifier")` -> "Specify either the name or the ARN of the investigation group that you want to view." The same sentence appears on Update/Delete/Get-Put-DeleteInvestigationGroupPolicy, and `InvestigationGroupIdentifier` (string, pattern with the `[\-_A-Za-z0-9]{1,512}` alternative) models the bare-name form as a well-formed identifier.

Handler/state: services/service-com-amazonaws-aiops-aiops/src/ops/get_investigation_group.rs line ~188 calls `store.group(&input.identifier, &region)`; services/service-com-amazonaws-aiops-aiops/src/state.rs `resolve_group_id()` begins with `let id = group_id_from_arn(identifier)?;` and its doc comment states "the service still resolves no group from one ... A name is therefore not an address: only the ARN the create handed back is." No name index exists on `ComAmazonawsAiopsAiopsState` (only `investigation_groups: BTreeMap<id, group>`).

Test coverage: the divergence is currently ASSERTED, not missing — tests/all/investigation_group_singleton.rs::only_the_arn_resolves_a_group feeds "the group's own name" and expects 404 ResourceNotFoundException for all six identifier operations, and tests/all/error_message_rules.rs (~line 613) asserts "The bare name is not an address". A fix must update those expectations alongside the resolver.

## Steps to Reproduce

1. Boot the emulator and create a group: POST /investigationGroups {"name":"my-group","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} -> 201 with an investigation-group ARN
2. GET /investigationGroups/my-group (the documented name form of `identifier`) -> emulator answers 404 ResourceNotFoundException; real AWS returns 200 with the group body
3. GET /investigationGroups/<url-encoded ARN> -> 200, showing the group exists and only the ARN form resolves
4. GET /investigationGroups -> the group is listed under the name `my-group`, contradicting the 404 above
