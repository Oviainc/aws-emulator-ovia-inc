Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 804429c0e5267423598a5b7dbc8d0525cc046b43

# ListRules' ResourceTags filter does not apply the crate's own "no value == the empty value" normalisation, so it hides the very rules the resource-tag quota counts as selecting on the same pair

_Filed: 2026-10-28T02:37:59Z_
_Operation: ListRules | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `804429c0e5267423598a5b7dbc8d0525cc046b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state.rs` states one identity for a resource tag pair — the key together with the value, where a tag carrying NO value IS the empty value — and the five-rules-per-pair quota (`exceeds_resource_tag_quota`, reached from `CreateRule` and `UpdateRule`) is built on it through `resource_tag_pair`, which normalises an absent `ResourceTagValue` to `""`.

The `ListRules` resource-tag filter in the same module skips that normalisation. `has_resource_tags` -> `contains_resource_tag` compares the OPTION, not the normalised value: a filter naming `ResourceTagValue: ""` requires the stored tag to carry `Some("")`, so it does not match a rule created with the value omitted — while the quota counter, in the same file, has just told the caller those two pairs are the SAME pair by refusing a create with HTTP 402.

So the two halves of the crate disagree about one input the model allows (`ResourceTagValue` is optional, length 0..256):
* The quota refuses a 6th rule selecting on `{Key: "ledger", Value: ""}` because five rules already select on `{Key: "ledger"}` (value omitted) — the pairs are one pair.
* `ListRules` filtered by `{Key: "ledger", Value: ""}` reports NONE of those five rules — the pairs are two pairs. The caller cannot see the rules that blocked it.

The mirror half of the asymmetry: a filter that omits the value is treated as a wildcard over every value of the key, so one `ListRules` call reports rules the quota counts as selecting on several DIFFERENT pairs. Either reading may be defensible on its own, but the crate implements both at once, and only the quota side's reading is stated as the crate's rule.

The fix is the normalisation the crate already owns: compare resource tag pairs through `resource_tag_pair` (or an equivalent that maps an absent value to the empty value) on the filter path too, so the state a page selects on and the state the quota counts agree about what one pair is.

## Evidence

The rule the crate states for itself, and the helper the quota path uses — `src/state.rs:108-117`:

    /// A resource tag as the five-rules-per-pair quota identifies it: the key together with
    /// the value, where a tag that carries no value is the empty value (`ResourceTagValue`
    /// is optional and bounded at `{0,256}`, so "no value" and "the empty value" select the
    /// same resources).
    fn resource_tag_pair(tag: &ResourceTag) -> (&str, &str) {
        (
            tag.resource_tag_key.as_str(),
            tag.resource_tag_value.as_deref().unwrap_or_default(),
        )
    }

The quota counter is built on it — `src/state.rs:1084-1098`:

        let mut counted: BTreeSet<(&str, &str)> = BTreeSet::new();
        tags.iter().any(|tag| {
            let pair = resource_tag_pair(tag);
            counted.insert(pair)
                && self
                    .rules_in_region(region)
                    ...
                    .filter(|rule| {
                        rule.resource_tags
                            .iter()
                            .any(|held| resource_tag_pair(held) == pair)
                    })

reached from `src/ops/create_rule.rs:312` (and `src/ops/update_rule.rs:299`):

        if store.exceeds_resource_tag_quota(&region, &resource_tags, None) {

The filter path skips the normalisation — `src/state.rs:504-513`:

    /// A tag matches when the keys are equal and, where the filter names a value, the
    /// values are equal too; a filter that names no value matches any value for the key.
    fn contains_resource_tag(tags: &[ResourceTag], wanted: &ResourceTag) -> bool {
        tags.iter().any(|tag| {
            tag.resource_tag_key == wanted.resource_tag_key
                && wanted
                    .resource_tag_value
                    .as_ref()
                    .is_none_or(|value| tag.resource_tag_value.as_ref() == Some(value))
        })
    }

(stored `resource_tag_value: None` vs a filter's `Some("")` -> `None == Some("")` -> no match), used by `src/state.rs:465-468` (`has_resource_tags`) and called from `src/ops/list_rules.rs:227-231`:

            if !rule.has_resource_tags(&resource_tag_filter)
                || !rule.has_exclude_resource_tags(&exclude_resource_tag_filter)
            {
                continue;
            }

Test coverage of the gap: none. `tests/all/resource_tag_pair_quota.rs:74-83` keeps the two forms on SEPARATE keys —

            Pair {
                case: "a pair whose request names no value",
                key: "cost-centre".to_string(),
                value: None,
            },
            Pair {
                case: "a pair with an empty value",
                key: "ledger".to_string(),
                value: Some(String::new()),
            },

and its read-back helper filters with exactly the form the rules were created with (`tests/all/resource_tag_pair_quota.rs:143-154`):

    /// How many rules `pair` currently selects, read back through `ListRules`.
    async fn rules_selecting(...)
        let (status, body) = list_rules(
            router,
            json!({ "ResourceType": resource_type, "ResourceTags": [pair.as_json()] }),

so no test ever crosses an omitted value with an empty one.

## Steps to Reproduce

1. POST /rules five times (MAX_RULES_PER_RESOURCE_TAG) with {"ResourceType":"EBS_SNAPSHOT","RetentionPeriod":{"RetentionPeriodValue":7,"RetentionPeriodUnit":"DAYS"},"ResourceTags":[{"ResourceTagKey":"ledger"}]} — the value member is omitted, which the model allows. Each answers 201.
2. POST /rules a sixth time with ResourceTags [{"ResourceTagKey":"ledger","ResourceTagValue":""}] — answered 402 ServiceQuotaExceededException "The request would exceed the quota of 5 retention rules per resource tag key and value pair": the service has just declared that ('ledger','') is the same pair the five stored rules select on.
3. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","ResourceTags":[{"ResourceTagKey":"ledger","ResourceTagValue":""}]} — expected the five rules the 402 says select on that pair; actual "Rules": [] (contains_resource_tag compares None against Some("")).
4. POST /list-rules with {"ResourceType":"EBS_SNAPSHOT","ResourceTags":[{"ResourceTagKey":"ledger"}]} after also creating a rule selecting on {"ResourceTagKey":"ledger","ResourceTagValue":"q3"} — the page reports rules the quota counts as two different pairs, the other half of the same asymmetry.
