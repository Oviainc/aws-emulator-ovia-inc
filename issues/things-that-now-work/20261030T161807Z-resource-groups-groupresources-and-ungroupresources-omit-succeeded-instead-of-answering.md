Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-group-resources-type-mismatch-roundtrip
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-group-resources-type-mismatch-roundtrip
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|GroupResources|2xx->2xx|$.Succeeded
Issue-Key: resource-groups|UngroupResources|2xx->2xx|$.Succeeded
Issue-Commit: 45d3476fddc0c303f86586399cfbfaaac87d2cc4

# GroupResources and UngroupResources omit Succeeded instead of answering an empty list

> _Note: this issue was identified against the code base at commit `45d3476fddc0c303f86586399cfbfaaac87d2cc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GroupResources

## Description
Both static-membership mutators of resource-groups drop their `Succeeded` member from the 200 response whenever the call moved no ARN. The recorded AWS responses carry `"Succeeded": []` in exactly that case, so a client that reads `Succeeded` unconditionally sees a present-but-empty list on AWS and an absent member here. Both causes are the SAME defect: the shared helper `crate::state::reported_list` collapses an empty `Vec` to `None` on the way out, and the two handlers' output construction comments justify that from the input-side `@length min 1` of `ResourceArnList`. Response shapes are serialized, not length-validated, so AWS emits the empty list regardless of that constraint; the helper's premise is the bug and it must stop rewriting an empty report as an omission for these two members.

Cause 1 of 2: GroupResources answers 200 with no `Succeeded` member when every requested ARN was refused by the group's configuration type (resource-type mismatch), so nothing was added. Fixture `resource-groups-group-resources-type-mismatch-roundtrip`, step `task#3`.
  AWS: Smithy `GroupResourcesOutput$Succeeded` targets `ResourceArnList` (list, member `ResourceArn`); the member is optional in the shape and is documented as "A list of Amazon resource names (ARNs) of the resources that this operation successfully added to the group." The model does not state whether the empty case is emitted or omitted, so this is recorded behavior only: the recording answers HTTP 200 with `Succeeded` PRESENT and equal to `[]`, alongside the populated `Failed` list naming the refused ARNs. No error code or message is involved — the status is 200 on both sides.
  Emulator: HTTP 200, `Succeeded` absent from the JSON body (replay detail: `$.Succeeded: missing (expected [], got None)`).
  Why: an SDK or script that does `len(resp["Succeeded"])`, iterates it, or compares it against the ARNs it submitted gets a KeyError / null dereference against the emulator on precisely the interesting path — the partial/total-failure path the fixture exercises — while the same code works against AWS. It also makes "added nothing" indistinguishable on the wire from "the field was not reported".
  Fix: the GroupResources handler's output construction, services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs (the `succeeded: crate::state::reported_list(succeeded)` assignment and the comment above it), and the shared `crate::state::reported_list` helper it calls — emit `Some(succeeded)` so an empty report serializes as `[]`.
  Traces: resource-groups-group-resources-type-mismatch-roundtrip#task#3.

Cause 2 of 2: UngroupResources answers 200 with no `Succeeded` member when none of the requested ARNs was held by the group, so nothing was removed. Fixture `resource-groups-group-resources-type-mismatch-roundtrip`, step `task#5`.
  AWS: Smithy `UngroupResourcesOutput$Succeeded` targets `ResourceArnList`, documented as "A list of resources that were successfully removed from the group by this operation." Again the model does not pin the empty case, so recorded behavior only: HTTP 200 with `Succeeded` present as `[]`.
  Emulator: HTTP 200, `Succeeded` absent (replay detail: `$.Succeeded: missing (expected [], got None)`).
  Why: same client breakage as cause 1, on the idempotent-repeat / nothing-to-remove path that callers use to confirm an ungrouping.
  Fix: services/service-com-amazonaws-resourcegroups-ardi/src/ops/ungroup_resources.rs output construction (`succeeded: crate::state::reported_list(succeeded)`), via the same helper.
  Traces: resource-groups-group-resources-type-mismatch-roundtrip#task#5.

Note on an existing open issue: 20261029T013351Z-resource-groups-ungroupresources-wrong-behavior--r2.md argues the OPPOSITE — that `Succeeded: []` is inadmissible because `ResourceArnList` is `@length min 1` — and it does not carry a replay key, so it does not cover these steps. The recorded AWS wire contradicts it: `@length` constrains inputs the service accepts, not what it serializes back, and AWS answers `[]`. Whoever fixes this should close that issue as resolved-by-contradiction rather than apply it, and keep tests/all/ungroup_resources.rs from re-asserting the omission (it currently accepts either form).

## Evidence
Replay of trace set simparity-wire-aws-v1 (9 fixtures, 57 steps, 46 passed, 11 mismatched):

1) divergence_key `service=resource-groups|GroupResources|status=200->200|keys=$.Succeeded`
   fixture_id `resource-groups-group-resources-type-mismatch-roundtrip`, step_key `task#3`
   expected_status 200, actual_status 200
   detail: `$.Succeeded: missing (expected [], got None)`

2) divergence_key `service=resource-groups|UngroupResources|status=200->200|keys=$.Succeeded`
   fixture_id `resource-groups-group-resources-type-mismatch-roundtrip`, step_key `task#5`
   expected_status 200, actual_status 200
   detail: `$.Succeeded: missing (expected [], got None)`

Expected (AWS, recorded): 200 with `"Succeeded": []` present in the body.
Actual (emulator): 200 with the `Succeeded` key absent from the body.
No status, error code or error message differs — this is purely an omitted response member.

Model (smithy_query query="shape" shape="GroupResourcesOutput,UngroupResourcesOutput,ResourceArnList" on services/service-com-amazonaws-resourcegroups-ardi/smithy-model/com-amazonaws-resourcegroups-ardi.json):
  `ResourceArnList`: {"type":"list","length":{"min":1,"max":10},"member":{"target":"ResourceArn"}}
  `GroupResourcesOutput$Succeeded` / `UngroupResourcesOutput$Succeeded`: optional, target `ResourceArnList`.

Source of the omission, services/service-com-amazonaws-resourcegroups-ardi/src/ops/group_resources.rs:295-299:
```
        Ok(GroupResourcesOutput {
            // `Succeeded` targets `ResourceArnList`, whose declared length starts
            // at one, so a call that added no membership cannot say so with an
            // empty list: the member is left out of the answer instead.
            succeeded: crate::state::reported_list(succeeded),
```
The UngroupResources handler builds its output through the same `crate::state::reported_list`, so both operations omit the member on the same condition.

## Steps to Reproduce
Cause 1 (GroupResources), fixture resource-groups-group-resources-type-mismatch-roundtrip step task#3:
1. CreateGroup with a static-membership service configuration whose type accepts only one resource type, e.g. `{"Name":"<group>","Configuration":[{"Type":"AWS::EC2::CapacityReservationPool"},{"Type":"AWS::ResourceGroups::Generic","Parameters":[{"Name":"allowed-resource-types","Values":["AWS::EC2::CapacityReservation"]}]}]}`.
2. POST /group-resources with `{"Group":"<group>","ResourceArns":["<an ARN of a resource type the configuration does NOT allow>"]}`.
3. Observe the 200 body. AWS: `{"Succeeded": [], "Failed": [ ... the refused ARN ... ]}`. Emulator: `Failed` present, `Succeeded` key absent.

Cause 2 (UngroupResources), same fixture step task#5:
4. POST /ungroup-resources with `{"Group":"<group>","ResourceArns":["<an ARN the group does not hold>"]}`.
5. Observe the 200 body. AWS: `"Succeeded": []` present. Emulator: `Succeeded` key absent.

Re-replay both steps of fixture resource-groups-group-resources-type-mismatch-roundtrip to confirm a fix.
