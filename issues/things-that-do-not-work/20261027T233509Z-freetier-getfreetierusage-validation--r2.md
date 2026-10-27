Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 3d1fad8f4871dd362a3fc79086918715269856ce

# USAGE_PERCENTAGE filter accepts the non-finite spellings "NaN"/"inf" that its own decode cannot compare

_Filed: 2026-10-27T23:35:09Z_
_Operation: GetFreeTierUsage | Class: validation_

> _Note: this issue was identified against the code base at commit `3d1fad8f4871dd362a3fc79086918715269856ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`check_dimension` decodes every `Values` entry of a `USAGE_PERCENTAGE` comparison with `wanted.trim().parse::<f64>()` and refuses anything that does not parse, with the message "... is not a number, and ... compares numbers". Rust's `f64` parser, however, accepts the non-finite spellings `NaN`, `nan`, `inf`, `infinity`, `-inf` and `-infinity` — every one of which the model's `Value` shape admits (`length 1..20`, pattern `^[ a-zA-Z0-9\-\:\.\_\/\,\$\(\)]*$`), so they reach the handler.

The consequence is exactly the outcome this crate says it refuses rather than serves:

* `Values: ["NaN"]` is accepted and then compares false in BOTH arms of `matches_dimension` (`number >= NaN` is false, and `(number - NaN).abs() < f64::EPSILON` is false), so the request answers HTTP 200 with an empty `freeTierUsages` page for a comparison that can never select anything — while the sibling value `"half"`, `"."`, `"1,5"` or `"0x32"` is refused with the modelled `ValidationException`.
* `Values: ["-inf"]` with `GREATER_THAN_OR_EQUAL` is accepted and matches EVERY record (`number >= -inf`), so a value that names no magnitude silently behaves as "no filter at all".

The refusal branch the fix needs is already in this function; the parse simply lets a non-finite value through it. Whether a comparison is evaluable is, per the function's own doc, settled here before any record is read — a non-finite "magnitude" is not settled, it is deferred to a comparison that is false (or vacuously true) by construction.

## Evidence

The decode and its own refusal, `services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs:482-491`:
```
            let mut magnitudes = Vec::with_capacity(dimensions.values.len());
            for wanted in &dimensions.values {
                let Ok(magnitude) = wanted.trim().parse::<f64>() else {
                    return Err(validation(format!(
                        "filter.Dimensions.Values: {wanted} is not a number, and {key} compares \
                         numbers"
                    )));
                };
                magnitudes.push(magnitude);
            }
```
The rule that function states for itself, `get_free_tier_usage.rs:423-426`: "comparing magnitudes needs values that are magnitudes — none of which depends on a record, so all of it is settled here — and settled ONCE: what this function decodes is what [`matches_dimension`] then compares by."

The comparison a decoded NaN reaches, `get_free_tier_usage.rs:546-552`:
```
        ) => magnitudes.iter().any(|wanted| {
            if comparison.option == MATCH_GREATER_THAN_OR_EQUAL {
                number >= *wanted
            } else {
                (number - wanted).abs() < f64::EPSILON
            }
        }),
```

The sibling handler in this same crate states the rule the accepted NaN breaks, `services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/list_account_activities.rs:166-168`: "Every status named in the filter must be one the model declares; a value outside the enum selects nothing, so it is refused rather than served as an empty page."

Test coverage: `services/service-com-amazonaws-freetier-awsfreetierservice/tests/all/dimension_decode.rs:241` — `a_comparison_the_decode_cannot_read_is_refused_at_every_level` drives the rows `vec!["half"], vec![" "], vec!["."], vec!["1,5"], vec!["0x32"], vec!["50 50"], vec!["fifty", "50"], vec!["50", "fifty"], vec!["0", "25", "nearly all"]` (lines 267-290) and asserts HTTP 400 `ValidationException` for each, nested inside `And`/`Or`/`Not` too. No row spells a non-finite value, so the gap is uncovered. The model admits such a value: `smithy_query(query="shape", shape="Value")` reports `"length": {"min": 1, "max": 20}, "pattern": "^[ a-zA-Z0-9\\-\\:\\.\\_\\/\\,\\$\\(\\)]*$"` — `NaN`, `nan`, `inf`, `infinity`, `-inf` all satisfy both (only a leading `+` is excluded by the pattern).

## Steps to Reproduce

1. POST / against the emulator with `content-type: application/x-amz-json-1.0` and `X-Amz-Target: AWSFreeTierService.GetFreeTierUsage`, body `{"maxResults":1000,"filter":{"Dimensions":{"Key":"USAGE_PERCENTAGE","Values":["half"],"MatchOptions":["EQUALS"]}}}` — observe HTTP 400 `ValidationException` ("half is not a number, and USAGE_PERCENTAGE compares numbers"), the behaviour `tests/all/dimension_decode.rs:241` pins.
2. Repeat with `"Values":["NaN"]`: expected the same refusal, since the value names no magnitude a record can equal; actual HTTP 200 with `"freeTierUsages": []` — the comparison was accepted and then matched nothing, because `(number - NaN).abs() < f64::EPSILON` is false for every record.
3. Repeat with `"Values":["inf"],"MatchOptions":["GREATER_THAN_OR_EQUAL"]`: actual HTTP 200 with an empty page for the same reason.
4. Repeat with `"Values":["-inf"],"MatchOptions":["GREATER_THAN_OR_EQUAL"]`: actual HTTP 200 listing EVERY stored usage record, i.e. the filter is silently equivalent to sending no filter at all.
5. Compare against `"Values":["50"]`, which is read as a magnitude and selects exactly the records whose percentage compares equal — showing the accepted non-finite spellings are the only values that pass the decode without being comparable.
