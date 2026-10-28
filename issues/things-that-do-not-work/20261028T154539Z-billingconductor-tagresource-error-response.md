Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 59590f018c84b50cc511c4837b9bc63c0d918b43

# TagResource names the same Tags member two different ways in ValidationException.Fields

_Filed: 2026-10-28T15:45:39Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `59590f018c84b50cc511c4837b9bc63c0d918b43`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` raises two modelled 400 `ValidationException`s about the SAME input member (`Tags`), and they spell that member differently in `ValidationException.Fields[0].Name` and in the clause they build from it:

* the reserved-`aws:`-namespace refusal names it `Tags` — the Smithy member name, which is what the helper's own doc says goes into `Fields`;
* the per-resource tag-cap refusal in the same handler names it `tags` (lowerCamel), and interpolates that spelling into the message as `Value at 'tags' failed to satisfy constraint: ...`.

The crate states one rule for this field, in the doc of the very helper both refusals go through: `field` / `member` is "the input member that carried the keys, which is what travels in `Fields`", and the sibling shared refusals illustrate it with the declared, PascalCase member names (`AccountIds`, `AccountGrouping.LinkedAccountIds`). Every other semantic refusal in this crate follows that rule — `AccountIds`, `NextToken`, `GroupBy`, `Arn`, `Name`, `Status`, `PricingRuleArns`, `BillingPeriodRange.ExclusiveEndBillingPeriod`. The tag-cap refusal is the single place that breaks it, so one client reading `Fields[0].Name` to locate the offending member gets `Tags` from one `TagResource` refusal and `tags` from the other.

The lowerCamel spelling is the one the BOUNDARY walk uses for constraint violations (`constraints::path::member`), but this refusal is not a boundary violation: it is a handler-raised semantic refusal built through `crate::state::validation_exception`, exactly like the reserved-key one immediately above it that uses `Tags`. Whichever spelling is right, the handler cannot be right both ways.

## Evidence

The stated rule, `services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs:3711-3713` (doc of `check_tag_keys_not_reserved`, the helper BOTH tagging operations and all four `Create*` operations share):

```
/// and cannot drift apart. `member` names the input member that carried the keys, which
/// is what travels in `Fields`, and the message is built from the request: the offenders
/// are sorted and deduplicated, so a map's iteration order never shows through the text.
```

and the same rule for the sibling shared refusal, `src/state.rs:4106-4108`, which spells out that the value is the DECLARED member name:

```
/// every operation that takes account ids answers it the same way; `field` names the
/// input member that carried the id (`AccountIds`, `AccountGrouping.LinkedAccountIds`,
/// …), which is what goes in `Fields`.
```

Side one — `src/ops/tag_resource.rs:177-182`, naming the member `Tags`:

```
        if let Err(refusal) = crate::state::check_tag_keys_not_reserved(
            "Tags",
            input.tags.keys().map(String::as_str),
        ) {
            return Err(TagResourceErrorModeled::ValidationException(refusal).into());
        }
```

Side two — `src/ops/tag_resource.rs:208-218`, the SAME member named `tags`, in the same handler, through the same `validation_exception` builder:

```
        if resulting > crate::state::MAX_TAGS_PER_RESOURCE {
            return Err(TagResourceErrorModeled::ValidationException(
                crate::state::validation_exception(
                    "FIELD_VALIDATION_FAILED",
                    "tags",
                    format!(
                        "Value at 'tags' failed to satisfy constraint: a resource must have \
                         no more than {} tags; this request would leave it with {resulting}",
                        crate::state::MAX_TAGS_PER_RESOURCE
                    ),
                ),
            )
            .into());
        }
```

Both refusals carry the SAME `Reason` (`FIELD_VALIDATION_FAILED`, `src/state.rs:3728` and `src/ops/tag_resource.rs:211`), so `Fields[0].Name` is the only thing that tells a client which member they are about — and it differs.

Test coverage: the `Tags` spelling IS pinned — `tests/all/reserved_tag_keys.rs:127` calls `assert_reserved_refusal(case, "Tags", offenders, &answered);` and that helper asserts at `tests/all/reserved_tag_keys.rs:54-58`:

```
    assert_eq!(
        body["Fields"][0]["Name"].as_str(),
        Some(member),
        "case {case:?}: expected the refusal to name the member {member:?}, got {body}"
    );
```

The `tags` spelling is pinned by NOTHING: the only test of the cap, `tests/all/tagging_surface.rs:855-878` (`a_resource_holds_at_most_the_modelled_number_of_tags`, the "one key past the cap" row), asserts the status (400), the error code (`ValidationException`), the `Reason` (`FIELD_VALIDATION_FAILED`) and that nothing was stored, and never reads `Fields`.

## Steps to Reproduce

1. grep -n "FIELD_VALIDATION_FAILED" services/service-com-amazonaws-billingconductor-awsbillingconductor/src/ops/tag_resource.rs services/service-com-amazonaws-billingconductor-awsbillingconductor/src/state.rs and observe both TagResource refusals use the same reason
2. Open src/ops/tag_resource.rs:177-182 and src/ops/tag_resource.rs:208-218 side by side: the same input member is passed to the shared ValidationException builder as "Tags" in one and "tags" in the other
3. Read src/state.rs:3711-3713 and src/state.rs:4106-4108: the crate's stated rule is that this argument is the input member that carried the value, illustrated with declared PascalCase member names
4. Create a pricing plan, TagResource it with 200 distinct keys, then TagResource it once more with one further key: the 400 body carries Reason FIELD_VALIDATION_FAILED and Fields[0].Name == "tags"
5. TagResource the same plan with a single key "aws:cost-center": the 400 body carries the same Reason FIELD_VALIDATION_FAILED but Fields[0].Name == "Tags"
6. Run `cargo test -p service-com-amazonaws-billingconductor-awsbillingconductor reserved_tag_keys tagging_surface`: only the "Tags" spelling is asserted anywhere, so aligning the cap refusal on it changes no existing expectation
