Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 617cb494a3d8f9d308b6478ef47b3df7665967e2

# Application-source identity key is an unescaped join, so two different tag-filter sources collide

_Filed: 2026-10-26T08:43:15Z_
_Operation: CreateScalingPlan | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `617cb494a3d8f9d308b6478ef47b3df7665967e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::application_source_key` renders a tag-filter `ApplicationSource` into a flat string by concatenating caller-supplied text with three unescaped delimiters: `=` between a filter key and its values, `,` between values, and `;` between filters. That string is the identity the single-plan-per-application-source rule and the `DescribeScalingPlans` `ApplicationSources` filter are both decided on (`plan_covering_source` compares keys for equality), so two application sources that are genuinely different — because a tag key or tag value itself contains `;`, `,` or `=`, all of which the modelled pattern allows — produce the SAME key and are treated as one source.

Two consequences, one root cause:

1. `CreateScalingPlan` (and `UpdateScalingPlan`, which uses the same helper) refuses a legitimate create with `ValidationException` "The application source is already covered by the scaling plan '<other>'" even though no stored plan claims that source. Example: plan A with `TagFilters: [{Key:"env",Values:["prod"]},{Key:"team",Values:["x"]}]` renders `tags:env=prod;team=x`; plan B with `TagFilters: [{Key:"env",Values:["prod;team=x"]}]` renders the identical `tags:env=prod;team=x`. Same for the value separator: `{Key:"tier",Values:["web","api"]}` and `{Key:"tier",Values:["api,web"]}` both render `tags:tier=api,web`.
2. `DescribeScalingPlans` filtering by `ApplicationSources` returns a plan whose application source is NOT the one asked for, because the filter is a membership test over the same colliding keys — a wrong success rather than an empty page.

The defect is that distinct inputs the model accepts are mapped onto one identity; it is not about any limit or wording.

## Evidence

The colliding encoding — services/service-com-amazonaws-autoscalingplans-anyscalescalingplannerfrontendservice/src/state.rs:264-283:
```
pub fn application_source_key(source: &ApplicationSource) -> Option<String> {
    ...
    let mut rendered: Vec<String> = filters
        .iter()
        .map(|filter| {
            let key = filter.key.clone().unwrap_or_default();
            let mut values = filter.values.clone().unwrap_or_default();
            values.sort();
            format!("{key}={}", values.join(","))
        })
        .collect();
    rendered.sort();
    Some(format!("tags:{}", rendered.join(";")))
}
```
No escaping of `=`, `,` or `;` anywhere, and the model permits all three inside the caller's text: `smithy_query(query="shape",shape="TagValues")` gives member `{"target": "XmlStringMaxLen256", "length": {"min": 1, "max": 256}, "pattern": "^[\\u0020-\\uD7FF...]*$"}` and `smithy_query(query="shape",shape="TagFilter")` gives `"Key": {"target": "XmlStringMaxLen128", ... "pattern": "^[\\u0020-\\uD7FF...]*$"}` — a character class that contains `\u003D` (=), `\u002C` (,) and `\u003B` (;).

That key IS the identity the rule is decided on — state.rs:234-244:
```
    pub fn plan_covering_source(
        &self,
        source_key: &str,
        excluding: Option<&str>,
    ) -> Option<&ScalingPlanRecord> {
        self.scaling_plans.values().find(|plan| {
            excluding != Some(plan.name.as_str())
                && application_source_key(&plan.application_source)
                    .is_some_and(|key| key == source_key)
        })
    }
```
whose doc one line above states the rule it is enforcing: "`ApplicationSource` is documented as \"You can create one scaling plan per application source\", so EVERY path that sets a plan's application source refuses a source this answers `Some` for" (state.rs:226-229).

The refusal the collision produces — src/ops/create_scaling_plan.rs:179-185:
```
        // "You can create one scaling plan per application source." No plan exists yet
        // under this name, so every stored plan is a competing claimant.
        if let Some(existing) = state.plan_covering_source(&source_key, None) {
            return Err(validation(
                crate::state::application_source_covered_message(&existing.name),
            ));
        }
```
and the wrong match it produces on the read path — src/ops/describe_scaling_plans.rs:215-218:
```
            if !wanted_source_keys.is_empty() {
                return crate::state::application_source_key(&plan.application_source)
                    .is_some_and(|key| wanted_source_keys.contains(&key));
            }
```

Test coverage: tests/all/application_source_exclusivity.rs:60-91 is the suite that pins source identity, and every tag value it uses is a plain word (`"prod"`, `"dev"`, `"web"`, `"api"`, `"worker"`, `"platform"`, `"staging"`) — no row contains `;`, `,` or `=`, so the collision is unpinned and a fix that disambiguates the encoding leaves all of these rows (and `names_covering` at :130) green.

## Steps to Reproduce

1. CreateScalingPlan with ScalingPlanName "plan-a", ApplicationSource {"TagFilters":[{"Key":"env","Values":["prod"]},{"Key":"team","Values":["x"]}]} and one valid ScalingInstruction — HTTP 200.
2. CreateScalingPlan with ScalingPlanName "plan-b", ApplicationSource {"TagFilters":[{"Key":"env","Values":["prod;team=x"]}]} and the same instruction shape (different resource id).
3. Observe HTTP 400 ValidationException "The application source is already covered by the scaling plan 'plan-a'", although plan-b's tag filter set is a different set of tags: state.rs:282 renders both as "tags:env=prod;team=x".
4. DescribeScalingPlans with ApplicationSources [{"TagFilters":[{"Key":"env","Values":["prod;team=x"]}]}] and observe plan-a returned, although plan-a does not have that application source (describe_scaling_plans.rs:215-218).
5. Repeat with the value separator: plan-a source {"TagFilters":[{"Key":"tier","Values":["web","api"]}]} vs plan-b source {"TagFilters":[{"Key":"tier","Values":["api,web"]}]} — same collision.
