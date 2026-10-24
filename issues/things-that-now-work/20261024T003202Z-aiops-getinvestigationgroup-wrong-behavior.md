Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 14fef70bd15ca29a08f279a21ebf0b06fea39359

# Identifier resolution ignores the request's Region, so a group in one Region is readable and mutable from another

_Filed: 2026-10-24T00:32:02Z_
_Operation: GetInvestigationGroup | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `14fef70bd15ca29a08f279a21ebf0b06fea39359`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Investigation groups are a regional resource: the aiops endpoint is per-Region (aiops.<region>.amazonaws.com) and a group created in us-west-2 exists only in that Region's data plane. The emulator already models this for ListInvestigationGroups (which filters by the signing Region) and for the one-group-per-Region create quota, but NOT for identifier resolution.

Expected: a GetInvestigationGroup request signed for us-east-1 that passes the ARN of a group created in us-west-2 cannot see that group — the us-east-1 endpoint holds no such resource, so the answer is a 404 ResourceNotFoundException (or a 400 ValidationException for the mismatched ARN Region), never the group body.

Actual: ComAmazonawsAiopsAiopsState::resolve_group_id resolves purely on an exact string match against the stored ARN and never compares the group's Region with crate::state::request_region(parts). A us-east-1-signed GetInvestigationGroup with a us-west-2 group ARN returns HTTP 200 with the full group body. The same shared resolver (group/group_mut/remove_group) is used by UpdateInvestigationGroup, DeleteInvestigationGroup, PutInvestigationGroupPolicy, GetInvestigationGroupPolicy, DeleteInvestigationGroupPolicy and the three tagging operations, so a request signed for any Region can also mutate or DELETE another Region's group — an observable state change made through an endpoint that should not reach that resource. The result is also internally inconsistent: ListInvestigationGroups in us-east-1 reports no groups while GetInvestigationGroup in us-east-1 returns one.

## Evidence

Spec/semantics: the Smithy model gives the service an ARN namespace and sigv4 name of `aiops` with regional endpoints, and InvestigationGroupArn embeds the Region (^arn:...:aiops:[a-zA-Z0-9-]*:[0-9]{12}:investigation-group/[A-Za-z0-9]{16}$) — a regional service does not serve another Region's resource. Handler/state: services/service-com-amazonaws-aiops-aiops/src/state.rs, resolve_group_id() filters only on `group.arn == identifier` (no Region comparison), while list_investigation_groups.rs reads `crate::state::request_region(ctx.parts())` and filters `group.region == region`, and create_investigation_group.rs enforces group_count_in_region(&region). Test coverage: tests/all/investigation_group_singleton.rs::only_the_arn_resolves_a_group varies ARN fields (region/account/partition/id inside the ARN) but every request is signed for the same REGION; tests/all/investigation_groups.rs::the_same_name_in_another_region_is_a_different_group reads each group with its own Region. No test signs a request for Region B and passes Region A's ARN, so this path is unasserted.

## Steps to Reproduce

1. Boot the emulator and sign a CreateInvestigationGroup request for us-west-2: POST /investigationGroups {"name":"ig","roleArn":"arn:aws:iam::000000000000:role/AIOpsAssistantRole"} -> 201 with arn:aws:aiops:us-west-2:000000000000:investigation-group/<id>
2. Sign a GetInvestigationGroup request for us-east-1 with that us-west-2 ARN: GET /investigationGroups/<url-encoded us-west-2 ARN> -> emulator answers 200 with the group body; the regional us-east-1 endpoint should not resolve it
3. Sign ListInvestigationGroups for us-east-1: GET /investigationGroups -> empty list, contradicting the 200 above
4. Sign a DeleteInvestigationGroup request for us-east-1 with the same us-west-2 ARN -> emulator answers 200 and the group is gone from us-west-2
