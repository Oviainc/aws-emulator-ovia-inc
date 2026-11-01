Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# UpdateDistribution replaces customErrorResponses verbatim, so the documented errorCachingMinTTL default of 10 seconds is dropped on the update path too

_Filed: 2026-10-28T17:38:17Z_
_Operation: UpdateDistribution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDistribution` takes the whole `customErrorResponses` list the request carries, checks only its LENGTH, and assigns it straight onto the stored distribution. An entry that omits `errorCachingMinTTL` therefore replaces whatever the distribution held with an entry that reports no caching window, although the member's modelled documentation states the service applies a default of 10 seconds when the request specifies none.

The same handler DOES materialise `origin.responseTimeout`'s documented 30-second default a hundred lines earlier, so the two documented defaults of the same request are treated inconsistently inside one file. (The create path has the identical defect; it is filed separately against CreateDistribution because the fix is in a different file.)

## Evidence

The model states the default itself — smithy_query(query="shape", shape="com.amazonaws.lightsail#DistributionCustomErrorResponse"), member `errorCachingMinTTL`:
```
"The minimum time, in seconds, that the distribution caches the custom error response before requesting the object again from the origin. If you don't specify a value, the default is 10 seconds."
```

The handler checks only the list length — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_distribution.rs:289-300:
```
        if let Some(custom_error_responses) = &input.custom_error_responses {
            if custom_error_responses.len()
                > crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
            {
                return Err(invalid(format!(
                    "customErrorResponses carries {} entries, more than the {} one distribution \
                     holds",
```
and then assigns the list verbatim — update_distribution.rs:435-437:
```
            if let Some(custom_error_responses) = input.custom_error_responses {
                distribution.custom_error_responses = custom_error_responses;
            }
```
reported back verbatim — src/state.rs:12472:
```
            custom_error_responses: Some(self.custom_error_responses.clone()),
```

The SAME handler materialises the other documented default of the same request — update_distribution.rs:248-249:
```
                let response_timeout = match origin.response_timeout {
                    None => crate::state::DEFAULT_ORIGIN_RESPONSE_TIMEOUT_SECONDS,
```

The crate's stated rule for nested structures — src/state.rs:9874-9876:
```
/// The health check `request` asks for, with every member it left out filled in
/// from that member's documented default, or a refusal naming the member that is
/// outside its documented range.
```

Test coverage does NOT pin the defect: tests/all/distribution_update.rs:183-188 and 280-306 always send `errorCachingMinTTL` explicitly, and the generated bound-check rows at tests/all/distribution_update.rs:688-692 send `responsePagePath`/`errorCachingMinTTL` for every entry, so the omitted-member form is never exercised.

## Steps to Reproduce

1. Against the us-east-1 endpoint, create an instance "origin-1" in us-east-1a and a distribution "cdn-1" over it with customErrorResponses=[{"errorCode":404,"responseCode":"404","responsePagePath":"/404.html","errorCachingMinTTL":60}].
2. Call UpdateDistribution with distributionName="cdn-1" and customErrorResponses=[{"errorCode":403,"responseCode":"403","responsePagePath":"/denied.html"}] — errorCachingMinTTL OMITTED.
3. Call GetDistributions: the single entry reports no `errorCachingMinTTL`, where the model says the service applies 10.
4. Compare with the same call sending origin={"name":"origin-1"} and no `responseTimeout`: the stored origin comes back with responseTimeout 30, from src/ops/update_distribution.rs:248-249.
