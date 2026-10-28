Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 2a79338fc5a5c4ae4fe5ea21e80da32dd32c19f2

# Tag-filter and metric-dimension identity keys sort but never deduplicate, so two spellings of one set render two different identities

_Filed: 2026-10-28T17:21:02Z_
_Operation: CreateScalingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `2a79338fc5a5c4ae4fe5ea21e80da32dd32c19f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::application_source_key` is the single value the crate decides the documented "one scaling plan per application source" rule on, and `crate::state::target_metric_identity` is the single value it decides the documented "unique scaling metric" rule (and the UpdateScalingPlan merge) on. Both are built from caller-supplied lists that the doc of each helper calls a SET, and both helpers `sort()` the rendered parts but never deduplicate them. `ApplicationSource$TagFilters`, `TagFilter$Values` and `MetricDimensions` are plain Smithy `list` shapes with no `@uniqueItems`, so a duplicate entry reaches the handler untouched, and `validate_application_source` / `validate_target_tracking_configurations` only bound the entry COUNTS — nothing rejects or collapses a repeat.

Consequences, all from the same root cause:

1. Application-source exclusivity is bypassable by re-spelling the source. A plan created over `TagFilters: [{Key:"env",Values:["prod"]}]` is keyed `tags:env=prod`; a second plan created over `TagFilters: [{Key:"env",Values:["prod"]},{Key:"env",Values:["prod"]}]` is keyed `tags:env=prod;env=prod`, and `plan_covering_source` compares keys for EQUALITY, so the second create is accepted. The same holds for `Values: ["prod","prod"]` (key `tags:env=prod,prod`). Two stored plans then cover the one application the rule exists to keep to a single plan, on both write paths (CreateScalingPlan and UpdateScalingPlan share the helper).
2. The same mismatch makes the `DescribeScalingPlans` `ApplicationSources` filter miss: a request selecting by the duplicated spelling of a tag set reports no plan even though a stored plan covers that set, because the filter is the same key equality.
3. `TargetTrackingConfigurations` uniqueness is bypassable the same way: two configurations on one instruction that track the same customized metric, one spelling its `Dimensions` with a repeated entry, render `N/M/Sum[a=1]` and `N/M/Sum[a=1,a=1]`, so the `BTreeSet` uniqueness check accepts both. The UpdateScalingPlan merge keyed on the same identity then also fails to pair a re-sent configuration with the one in force whenever the dimension list is re-spelled, so it re-points a target value the crate's own merge rule says it must keep.

## Evidence

The helper's own doc states the rule the code under it breaks — `src/state.rs:528-535`:

    /// The identity of an `ApplicationSource`, used to enforce "You can create one
    /// scaling plan per application source" (`ApplicationSource` member doc).
    ///
    /// A tag-filter source is identified by the SET of tags it names, so the filters and
    /// each filter's values are ordered before they are rendered — two spellings of one set
    /// are one source — and each of them is escaped, ...

The code only orders; it never deduplicates — `src/state.rs:550-567`:

    let mut rendered: Vec<String> = filters
        .iter()
        .map(|filter| {
            let key = escape_key_part(filter.key.as_deref().unwrap_or_default());
            let mut values: Vec<String> = filter
                .values
                ...
                .map(escape_key_part)
                .collect();
            values.sort();
            format!("{key}={}", values.join(","))
        })
        .collect();
    rendered.sort();
    Some(format!("tags:{}", rendered.join(";")))

The rule the key feeds compares keys for EQUALITY — `src/state.rs:459-461` and `src/state.rs:472-477`:

    /// `ApplicationSource` is documented as "You can create one scaling plan per
    /// application source", so EVERY path that sets a plan's application source refuses
    /// a source this answers `Some` for.
    ...
        self.scaling_plans.values().find(|plan| {
            excluding != Some(plan.name.as_str())
                && application_source_key(&plan.application_source)
                    .is_some_and(|key| key == source_key)
        })

and the refusal it drives — `src/ops/create_scaling_plan.rs:179-185`:

    // "You can create one scaling plan per application source." No plan exists yet
    // under this name, so every stored plan is a competing claimant.
    if let Some(existing) = state.plan_covering_source(&source_key, None) {
        return Err(validation(
            crate::state::application_source_covered_message(&existing.name),
        ));
    }

The second instance, same pattern — `src/state.rs:978-983` and `src/state.rs:1004`:

    /// A customized metric's parts are caller-supplied text under no modelled pattern at
    /// all ... so each is escaped on the way into the
    /// key and two different metrics cannot render one identity. The dimensions are a set,
    /// so they are ordered first; ...
    ...
    dimensions.sort();

and the uniqueness rule it feeds — `src/state.rs:971-972` and `src/state.rs:1174-1178`:

    /// "Each of these structures must specify a unique scaling metric and a target value
    /// for the metric" (`ScalingInstruction$TargetTrackingConfigurations`), ...
    ...
        if !metrics.insert(metric.clone()) {
            return Err(format!(
                "{at}: each target tracking configuration must specify a unique scaling \
                 metric, and '{metric}' is already specified"
            ));

The model lets duplicates reach the handler, and calls the list a set:
`smithy_query(query="shape",shape="ApplicationSource")` -> member `TagFilters`: `{"target": "TagFilters", "type": "list", "doc": "<p>A set of tags (up to 50).</p>"}`;
`smithy_query(query="shape",shape="TagValues")` -> `{"type": "list", "member": {"target": "XmlStringMaxLen256", ...}}` (no `uniqueItems`);
`smithy_query(query="shape",shape="MetricDimensions")` -> `{"type": "list", "member": "MetricDimension"}` (no `uniqueItems`).

Validation bounds only the counts, never the repeats — `src/state.rs:669-682` (`validate_application_source`) calls `check_entry_count("applicationSource.tagFilters", filters.len(), MAX_TAG_FILTERS)` and the per-filter `values` count, and nothing else inspects the entries.

Test coverage: `tests/all/identity_key_separators.rs` and `tests/all/application_source_exclusivity.rs` exercise this key, and `tests/all/tag_filter_entry_bounds.rs` the counts; none of them drives a DUPLICATED filter, a duplicated tag value or a duplicated metric dimension, so no existing test pins the current behaviour. Existing issues cover the opposite direction only — `20261026T084315Z-auto-scaling-plans-createscalingplan-wrong-behavior.md` ("Application-source identity key is an unescaped join, so two different tag-filter sources collide") — and `20261028T161832Z-auto-scaling-plans-createscalingplan-validation--r3.md` covers a TagFilter with no Key, not a repeated one.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "plan-a", ApplicationSource {"TagFilters":[{"Key":"env","Values":["prod"]}]} and one valid ScalingInstruction -> 200, ScalingPlanVersion 1.
2. CreateScalingPlan with ScalingPlanName "plan-b", ApplicationSource {"TagFilters":[{"Key":"env","Values":["prod"]},{"Key":"env","Values":["prod"]}]} and the same instruction -> observe 200. Expected a 400 ValidationException "The application source is already covered by the scaling plan 'plan-a'", because the two sources name the same set of tags.
3. Repeat step 2 with ApplicationSource {"TagFilters":[{"Key":"env","Values":["prod","prod"]}]} -> also 200, so a duplicated tag VALUE re-spells the source too.
4. DescribeScalingPlans with ApplicationSources [{"TagFilters":[{"Key":"env","Values":["prod","prod"]}]}] -> observe ScalingPlans [] even though "plan-a" covers that tag set.
5. CreateScalingPlan with one instruction whose TargetTrackingConfigurations hold two entries that both name CustomizedScalingMetricSpecification {"Namespace":"N","MetricName":"M","Statistic":"Sum","Dimensions":[{"Name":"a","Value":"1"}]} and {"Namespace":"N","MetricName":"M","Statistic":"Sum","Dimensions":[{"Name":"a","Value":"1"},{"Name":"a","Value":"1"}]} with different TargetValues -> observe 200. Expected the "each target tracking configuration must specify a unique scaling metric" refusal.
6. UpdateScalingPlan on that plan re-sending the configuration with the dimension list spelled the other way -> observe the stored TargetValue replaced by the request's, although crate::state::merge_scaling_instructions documents that the target value in force for a metric the resource already tracks is kept.
