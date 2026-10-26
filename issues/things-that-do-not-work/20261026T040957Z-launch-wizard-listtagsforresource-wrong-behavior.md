Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: b314e0f3433a6118e8049d188afadfe9e3c07a46

# ListTagsForResource reports an untagged deployment as an empty `tags` map, which the modelled `Tags` shape forbids

_Filed: 2026-10-26T04:09:57Z_
_Operation: ListTagsForResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b314e0f3433a6118e8049d188afadfe9e3c07a46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListTagsForResourceOutput$tags` targets `Tags`, a map the model constrains to `@length(min: 1, max: 200)`. When the resolved deployment carries no tags, the handler still serialises the member, producing `"tags": {}` — a `Tags` value of zero entries, i.e. a response that breaks the output member's own modelled minimum length.

This also contradicts the rule the rest of the crate applies to exactly the same stored field and exactly the same modelled shape. `Deployment::to_data()` — the projection `GetDeployment` reports, over the very same `deployment.tags` `BTreeMap` and the very same `Tags` shape — OMITS the member when the map is empty. The two specification-schema projections do the same thing for their own minimum-length collections, and `SpecificationField::allowed_values`' doc states the rule explicitly ("nothing is reported rather than an empty list"). So one reader of `deployment.tags` omits the empty map and the other emits it: the crate answers "this resource has no tags" two incompatible ways, and only one of them stays inside the model.

Note on test coverage: the round-trip test's intent ("the resource exists and carries no tags") is satisfied either way — it asserts `.is_empty()` — but its helper `listed_tags` panics when the member is absent, so that helper has to tolerate an absent `tags` member for the fix. The helper `deployment_tags` in the same file already tolerates it (`None => BTreeMap::new()`), which is the form GetDeployment produces.

## Evidence

Model (smithy_query query="operation" operation="ListTagsForResource"):
  "output": {"tags": {"target": "Tags", "length": {"min": 1, "max": 200}, "type": "map"}}
and `smithy_query query="shape" shape="Tags"`:
  "Tags": {"type": "map", "length": {"min": 1, "max": 200}, ...}
OPERATIONS.md:31 restates it: "**Output:** `tags` (map<string,string>, len 1..200)".

The handler, `services/service-com-amazonaws-launchwizard-launchwizard/src/ops/list_tags_for_resource.rs:165-170`:
```
// The tags come from the stored deployment, which is where every tagging call
// writes them. A deployment carrying none reports an empty map rather than no
// map at all: the answer is "this resource has no tags", not "no answer".
Ok(ListTagsForResourceOutput {
    tags: Some(crate::state::to_hash_map(&deployment.tags)),
})
```

The sibling reader of the SAME field through the SAME `Tags` shape, `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:744` (inside `Deployment::to_data`, what `GetDeployment` returns):
```
tags: (!self.tags.is_empty()).then(|| to_hash_map(&self.tags)),
```

The same rule again for the crate's other minimum-length output collections, `src/state.rs:291-292` and `src/state.rs:301`:
```
allowed_values: (!self.allowed_values.is_empty())
    .then(|| self.allowed_values.clone()),
...
conditionals: (!self.conditionals.is_empty()).then(|| {
```
with the stated reason at `src/state.rs:275-276`: "`DeploymentSpecificationsField$allowedValues`: empty when the field takes any `ValueString`, in which case nothing is reported rather than an empty list."

Tests: `tests/all/resource_tagging.rs:574-590` asserts only that the reported set `.is_empty()` ("Removing every remaining key leaves an empty set, which is still an answer: the resource exists and carries no tags"), but its helper `listed_tags` at `tests/all/resource_tagging.rs:224-228` panics if the member is absent, while `deployment_tags` at `tests/all/resource_tagging.rs:241-244` already accepts `None`.

## Steps to Reproduce

1. Start the emulator and create a deployment with no tags: POST /createDeployment {"workloadName":"SAP","deploymentPatternName":"SapHanaSingle","name":"untagged","specifications":{"KeyPairName":"kp","VpcId":"vpc-1"},"dryRun":false}
2. Note the returned deploymentId and build its ARN: arn:aws:launchwizard:us-east-1:<account>:deployment/<deploymentId>
3. GET /tags/<url-encoded resourceArn> — observe the body is {"tags":{}}, a `Tags` map with 0 entries, below the modelled minimum of 1
4. POST /getDeployment {"deploymentId":"<id>"} — observe `deployment` carries NO `tags` member at all for the same untagged record, i.e. the crate answers the same question the other way
