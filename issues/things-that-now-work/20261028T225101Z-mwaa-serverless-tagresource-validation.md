Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f08062a3928279f71031748dc453b175f9086c5b

# TagResource stores a tag key outside TagKey's modelled 1..128 length, the exact bound UntagResource enforces — so the tag can never be removed

_Filed: 2026-10-28T22:51:01Z_
_Operation: TagResource | Class: validation_

> _Note: this issue was identified against the code base at commit `f08062a3928279f71031748dc453b175f9086c5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Tags` is modelled as a map whose KEY targets `TagKey`, `@length(min: 1, max: 128)`. This crate re-declares that bound (`TAG_KEY_TRAITS`) and enforces it on `UntagResource$TagKeys`, where every element is walked against it — a test pins the refusal for a key below the minimum and a key one past the maximum. `TagResource` hands `Tags` over to the handler (`skip_validation = ["ResourceArn", "Tags"]`) and walks it as `Entries(&input.tags)` against `M_TAGS`, whose element constraints are `TAG_VALUE_TRAITS` only; the key position carries no constraint at all. So a key the model forbids is accepted and written straight into the stored tag map.

The consequence is state that can be written but not removed, and a response that breaks the key's own modelled length:

* `TagResource(Tags={"": "v"})` answers 200 and `workflow.tags.insert("", "v")` persists a key of length 0, below `TagKey`'s modelled minimum of 1.
* `ListTagsForResource` then reports a `Tags` map whose key violates `TagKey`'s `@length(min: 1)`.
* `UntagResource(TagKeys=[""])` — the only way to remove it — is refused 400 `ValidationException` naming `TagKeys`, because that path DOES apply `TAG_KEY_TRAITS`. The tag is unremovable through the modelled API for the lifetime of the workflow.

The same asymmetry holds at the other end of the bound: a key of 129..256 characters passes the `Tags` walk (the key is written with the map member's element constraints, i.e. `TagValue`'s 0..256, when it is walked at all) and is refused by `UntagResource`.

`CreateWorkflow` hands the same member over the same way (`members.push((&crate::state::M_TAGS, Entries(tags)))`), so a workflow can also be CREATED already holding an unremovable key. The root cause is single: the shadow `Tags` schema carries no key constraints, while the crate already has the `TagKey` trait map it needs and uses it on the untag path.

## Evidence

Model — `smithy_query(query="shape", shape="Tags,TagKeys")`:
```
"Tags": {"type": "map", "length": {"min": 0, "max": 50},
  "key":   {"target": "TagKey",   "length": {"min": 1, "max": 128}, "pattern": "^.*$"},
  "value": {"target": "TagValue", "length": {"min": 0, "max": 256}, "pattern": "^.*$"}}
"TagKeys": {"type": "list", "length": {"min": 1, "max": 50},
  "member": {"target": "TagKey", "length": {"min": 1, "max": 128}, "pattern": "^.*$"}}
```

The crate already declares the bound — src/state.rs:2003-2009:
```
/// `TagKey`: `@length(1..128)` plus `@pattern(^.*$)`.
static TAG_KEY_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(128))));
    traits.insert(Box::new(PatternTrait::new(r"^.*$")));
    traits
});
```

The side that ENFORCES it — src/state.rs:2075-2082:
```
/// `TagKeys`: `@length(1..50)` ENTRIES, with `TagKey`'s own constraints carried
/// for each element (`smithy_query(query="shape", shape="TagKeys")`).
static TAG_KEYS_TRAITS: ... {
    traits.insert(Box::new(LengthTrait::new(Some(1), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_KEY_TRAITS)));
```
reached by src/ops/untag_resource.rs:251-254:
```
    let violations = crate::state::validate_members(&[
        (&crate::state::M_RESOURCE_ARN, Text(&input.resource_arn)),
        (&crate::state::M_TAG_KEYS, Texts(&input.tag_keys)),
    ]);
```

The side that SKIPS it — src/state.rs:2064-2073:
```
/// `Tags`: `@length(0..50)` ENTRIES, with `TagValue`'s own constraints carried
/// for each value exactly as codegen carries them
/// (`smithy_query(query="shape", shape="Tags")`). A map KEY is not walked, which
/// is the framework's own rule, not a choice made here.
static TAGS_TRAITS: ... {
    traits.insert(Box::new(LengthTrait::new(Some(0), Some(50))));
    traits.insert(Box::new(ElementConstraintsTrait::new(&TAG_VALUE_TRAITS)));
```
(no `TAG_KEY_TRAITS` anywhere in this trait map), reached by src/ops/tag_resource.rs:273-276:
```
    let violations = crate::state::validate_members(&[
        (&crate::state::M_RESOURCE_ARN, Text(&input.resource_arn)),
        (&crate::state::M_TAGS, Entries(&input.tags)),
    ]);
```
and the unchecked key is then persisted — src/ops/tag_resource.rs:248-252:
```
        // "Adds tags to a … resource": the named keys are added or overwritten in place
        // and every other key the resource carries keeps the value it had.
        for (key, value) in input.tags {
            workflow.tags.insert(key, value);
        }
```
The same hand-over on the create path — src/ops/create_workflow.rs:488-490:
```
    if let Some(tags) = input.tags.as_ref() {
        members.push((&crate::state::M_TAGS, Entries(tags)));
    }
```

Test coverage — the untag refusal is PINNED, tests/all/resource_untagging.rs:567-576:
```
        (
            "a key below TagKey's minimum of 1 character".to_string(),
            vec![String::new()],
        ),
        (
            format!(
                "a key one past TagKey's maximum of {MAX_TAG_KEY_LEN} characters"
            ),
            vec!["k".repeat(MAX_TAG_KEY_LEN + 1)],
        ),
```
asserted at tests/all/resource_untagging.rs:581-590 (`expected HTTP 400` / `expected a ValidationException`). Nothing pins the tag side: tests/all/resource_tagging.rs:167-181 only exercises a key EXACTLY at the bound (`let longest_key = "k".repeat(MAX_TAG_KEY_LEN);` … "a key at TagKey's maximum of 128 …") and tests/all/validation_errors.rs:277-285 refuses only an over-long tag VALUE (`"Tags": {"big": "v".repeat(257)}` → field `Tags.big`). No suite sends an empty or over-long tag KEY to `TagResource` or `CreateWorkflow`.

## Steps to Reproduce

1. Boot the emulator and point an airflow-serverless client at it.
2. CreateWorkflow(Name='UnremovableTag', DefinitionS3Location={'Bucket':'defs','ObjectKey':'dags/d.yaml'}, RoleArn='arn:aws:iam::000000000000:role/wf-exec') and note the returned WorkflowArn.
3. TagResource(ResourceArn=<arn>, Tags={'': 'v'}) — observe HTTP 200, although the modelled `Tags` key targets `TagKey` with @length(min: 1).
4. ListTagsForResource(ResourceArn=<arn>) — observe the stored map reports a key of length 0, below TagKey's modelled minimum.
5. UntagResource(ResourceArn=<arn>, TagKeys=['']) — observe HTTP 400 ValidationException with a FieldList entry naming TagKeys (the refusal pinned by tests/all/resource_untagging.rs). The tag written in step 3 can never be removed.
6. Repeat steps 3-5 with a 200-character key: TagResource answers 200 and stores it, UntagResource answers 400 for the same key, so it is likewise unremovable.
7. Same via create: CreateWorkflow(..., Tags={'': 'v'}) succeeds, so a workflow can be created already holding a key no UntagResource call can name.
