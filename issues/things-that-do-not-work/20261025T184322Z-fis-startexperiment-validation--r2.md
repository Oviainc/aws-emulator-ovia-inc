Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1547d46e2415f486cf41cf952181485c961e01df

# Tag keys outside TagKey's modelled bounds are persisted by StartExperiment (and CreateExperimentTemplate), while TagResource refuses them

_Filed: 2026-10-25T18:43:22Z_
_Operation: StartExperiment | Class: validation_

> _Note: this issue was identified against the code base at commit `1547d46e2415f486cf41cf952181485c961e01df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The crate states, in one place, that `TagKey`'s modelled bounds are a HANDLER-side rule for this service, because the request validator never reaches a map's key target — and then applies that rule in exactly one of the three operations that take a tag map.

`TagResource` calls `crate::state::tag_keys_refusal("tags", ...)` before writing, so a key that is empty (which `TagKey`'s `pattern ^[\s\S]+$` does not admit) or longer than 128 characters is refused with 400 `ValidationException` and nothing is stored.

`StartExperiment` stores `input.tags` verbatim onto the experiment record (`start_experiment.rs:324` -> `ExperimentRecord::from_template` -> `tags: tags.unwrap_or_default()`) without ever consulting `tag_keys_refusal`, so `StartExperiment` with `tags={"": "x"}` or a 129-character key answers 200 and the out-of-bounds key is persisted on the experiment. Every later `GetExperiment`/`ListExperiments` then reports a `TagMap` whose key the model cannot hold, and a subsequent `TagResource` carrying the very same key on the very same resource is refused — so the two tag-writing paths of one resource disagree about what a tag key may be.

`CreateExperimentTemplate` has the identical gap on the same helper (`create_experiment_template.rs` builds the record with `ExperimentTemplateRecord::from_create_input(.., &input, ..)` and judges it only through `crate::state::template_refusal`, which checks roleArn, stopConditions, the experimentOptions enums, logConfiguration and the actions — never the tag keys). The one fix is to run the existing `tag_keys_refusal` helper over `input.tags` on both write paths; the `CreateExperimentTemplate` symptom is held in this issue rather than filed separately because that operation already carries an open `validation` issue.

Expected: a request carrying a tag key outside `TagKey`'s bounds is refused with `ValidationException` (both operations declare it) and nothing is persisted. Actual: 200, and the key is stored and reported.

## Evidence

Smithy model (smithy_query shape=TagMap): key target `TagKey` with `"length": {"min": 0, "max": 128}, "pattern": "^[\\s\\S]+$"` — so an empty key and a 129-character key are both inadmissible; `StartExperiment` and `CreateExperimentTemplate` both declare `ValidationException (HTTP 400)`.

The rule, stated once in this crate — services/service-com-amazonaws-fis-faultinjectionsimulator/src/state.rs:2668-2671:
  "/// The boundary validator captures a map's KEY without checking it (its VALUE target IS
   /// checked), so a request carrying a tag key outside `TagKey`'s modelled bounds reaches
   /// the handler. This is where those bounds are enforced, for every operation of this
   /// service that takes tag keys."
and the contrast with the value target, state.rs:308-310 (`TAG_VALUE_MAX_LENGTH`):
  "/// Nothing here enforces it: the boundary validator reaches a map's VALUE target, so a
   /// longer tag value is refused before any handler runs."
The framework confirms it — vendor/smithy-schema-server/src/constraints.rs:569-570: "Map *keys* are not covered (codegen does not capture key constraints yet)." and constraints.rs:1090-1091: "toggle the frame; the key itself is not a constrained leaf."

The operation that applies the rule — src/ops/tag_resource.rs:160-166:
  "// A map's KEY target is not reached by the boundary validator (its VALUE target
   // is), so `TagKey`'s modelled bounds are enforced here.
   if let Some(refusal) = crate::state::tag_keys_refusal("tags", input.tags.keys().map(String::as_str)) { return Err(TagResourceErrorModeled::ValidationException(refusal).into()); }"

The operations that skip it — src/ops/start_experiment.rs:319-327 persists the tags with no tag check in either `handle` (lines 357-389) or `start_under_guard` (lines 165-342):
  "let record = crate::state::ExperimentRecord::from_template(id.clone(), arn, &template, actions_mode, input.tags.clone(), now, permission_failure);"
and src/ops/create_experiment_template.rs:193-197, whose own comment claims only the map's SIZE was refused upstream:
  "// Everything the model CAN express (the required members, the lengths and patterns
   // on `clientToken`, `description` and `roleArn`, the 50-entry tag map) was already
   // refused at the boundary."
A grep of the crate shows `tag_keys_refusal`/`tag_key_refusal` is called from exactly one handler: src/ops/tag_resource.rs:163.

Test coverage: tests/all/resource_tagging.rs:529-567 pins the key bounds for `TagResource` only ("a tag key one character past TagKey's ceiling", "an empty tag key, which TagKey's pattern does not admit"), and its `new_resource` helper always creates the template/experiment with admissible tags (lines 148-180), so no test pins the create/start path's acceptance of a bad key and no comment or test states a reason for the divergence.

## Steps to Reproduce

1. Boot the emulator and point a botocore `fis` client at it.
2. Create a template: create_experiment_template(clientToken='t1', description='d', roleArn='arn:aws:iam::123456789012:role/fis', stopConditions=[{'source':'none'}], actions={'w':{'actionId':'aws:fis:wait','parameters':{'duration':'PT1M'}}}) — note its id.
3. start_experiment(clientToken='s1', experimentTemplateId=<id>, tags={'': 'x'}) — observe HTTP 200 instead of ValidationException; the empty tag key is accepted.
4. get_experiment(id=<experiment id>) — observe the response's `tags` carrying the empty key that `TagKey`'s pattern `^[\s\S]+$` does not admit.
5. tag_resource(resourceArn=<experiment arn>, tags={'': 'x'}) — observe 400 ValidationException for the SAME key the start just persisted.
6. Repeat with a 129-character key, and with create_experiment_template(..., tags={'k'*129: 'x'}) — both answer 200 and the key is readable through GetExperiment / GetExperimentTemplate.
