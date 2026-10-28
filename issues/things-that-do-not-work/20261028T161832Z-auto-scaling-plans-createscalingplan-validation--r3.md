Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d507b4490f793f529ee51b4fa9905ce30659ebfa

# A TagFilter that names no tag key is accepted as an application source, while the stack-ARN side of the same predicate refuses an empty value

_Filed: 2026-10-28T16:18:32Z_
_Operation: CreateScalingPlan | Class: validation_

> _Note: this issue was identified against the code base at commit `d507b4490f793f529ee51b4fa9905ce30659ebfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::validate_application_source` — the one rule `CreateScalingPlan`, `UpdateScalingPlan` and `DescribeScalingPlans` all decide an `ApplicationSource` by — normalises the CloudFormation-stack side of the alternative but not the tag-filter side.

On the stack side, `named_stack_arn` throws away a value that is present but empty, so `{"CloudFormationStackARN": ""}` "names nothing" and the source is refused. On the tag side, `names_tag_filters` inspects only the LENGTH of the list, and `application_source_key` renders a filter with no `Key` as the empty key-part. The result is two contradictory answers to the same question:

* `ApplicationSource: {"TagFilters": []}` -> refused, "ApplicationSource must specify either CloudFormationStackARN or TagFilters";
* `ApplicationSource: {"TagFilters": [{}]}` -> ACCEPTED, stored, and given the application-source identity `tags:=`.

Neither source names a tag, so the second is a scaling plan whose application source identifies no application. The identity key it gets also feeds the documented single-plan-per-application-source rule (`plan_covering_source`), so the empty part becomes a real component of that key: a source spelled `[{"Key":"env","Values":["prod"]}, {}]` keys as `tags:=;env=prod` instead of `tags:env=prod`, i.e. a filter that names no tag changes which application a plan is held to claim.

The empty key-part the key is built from is also a tag key the model itself forbids: `TagFilter$Key` carries `length {min: 1, max: 128}`, so an empty key can never arrive on the wire, yet `filter.key.as_deref().unwrap_or_default()` manufactures one for a filter that simply omitted it.

## Evidence

Both sides of the asymmetry are in one module, `services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs`:

Stack side (state.rs:582-591) — empties are normalised away, and the doc says this is the single place that decides it:
```
/// A `CloudFormationStackARN` that is absent, or present but empty, names nothing. This
/// is the single place that decides what "names a stack" means, so the identity key
/// ([`application_source_key`]), the exclusivity rule that key feeds and the ARN-format
/// check in [`validate_application_source`] cannot disagree about one source.
fn named_stack_arn(source: &ApplicationSource) -> Option<&str> {
    source
        .cloud_formation_stack_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
}
```

Tag side (state.rs:570-577) — only the list's emptiness is considered, never a filter's:
```
/// Whether a tag-filter list identifies anything: a filter list that is absent or
/// empty selects nothing.
fn names_tag_filters(source: &ApplicationSource) -> bool {
    source
        .tag_filters
        .as_ref()
        .is_some_and(|filters| !filters.is_empty())
}
```

The identity key manufactures an empty tag key (state.rs:553):
```
            let key = escape_key_part(filter.key.as_deref().unwrap_or_default());
```
and the accepted/refused split is decided at state.rs:689-701:
```
    match (stack_arn.is_some(), names_tag_filters(source)) {
        (true, true) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters, not both"
...
        (false, false) => Err(
            "ApplicationSource must specify either CloudFormationStackARN or TagFilters"
```

Model side — an empty key cannot be sent, so the key-part state.rs:553 builds is one the model excludes: `smithy_query(query="shape",shape="TagFilter")` gives `"Key": {"target": "XmlStringMaxLen128", "length": {"min": 1, "max": 128}, "doc": "<p>The tag key.</p>"}` on a shape documented `"<p>Represents a tag.</p>"`.

Test coverage: no test accepts a keyless filter. The only tests that send one — `tests/all/tag_filter_entry_bounds.rs:266-296` (`let keyless = json!({ "Values": ["prod"] });`) — send it inside sources that are refused anyway (past the 50-entry bound, or beside a stack ARN), and the one assertion there only requires `(400, "ValidationException")` with a message different from the length sentence, so refusing a keyless filter leaves it passing. `tests/all/scaling_plan_lifecycle.rs:886` pins the neighbouring case `("empty tag filter list", "fifth", json!({ "TagFilters": [] }))` as refused.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AnyScaleScalingPlannerFrontendService.CreateScalingPlan` and body `{"ScalingPlanName":"p1","ApplicationSource":{"TagFilters":[{}]},"ScalingInstructions":[<one valid instruction>]}` — observe HTTP 200 with `{"ScalingPlanVersion":1}`: a plan whose application source names no tag is stored.
2. Repeat with `"ApplicationSource":{"TagFilters":[]}` — observe HTTP 400 `ValidationException` "ApplicationSource must specify either CloudFormationStackARN or TagFilters", and with `"ApplicationSource":{"CloudFormationStackARN":""}` — the same refusal. The same "names no tag/stack" input is refused in two spellings and accepted in a third.
3. Call `DescribeScalingPlans` with no selector and see plan `p1` reported with `ApplicationSource: {"TagFilters":[{}]}`.
4. Create plan `p2` with `"ApplicationSource":{"TagFilters":[{"Key":"env","Values":["prod"]}]}`, then create plan `p3` with `"ApplicationSource":{"TagFilters":[{"Key":"env","Values":["prod"]},{}]}` — `p3` is accepted because the keyless filter renders an extra `=` part in the identity key (`tags:=;env=prod` vs `tags:env=prod`), so the keyless filter alone decides whether the single-plan-per-application-source refusal fires.
