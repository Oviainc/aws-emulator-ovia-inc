Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# DistributionCustomErrorResponse$errorCachingMinTTL's documented default of 10 seconds is never materialised, unlike every other documented default this handler fills in

_Filed: 2026-10-28T17:38:17Z_
_Operation: CreateDistribution | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateDistribution` accepts `customErrorResponses`, checks only the LIST LENGTH, and then stores every entry verbatim (`custom_error_responses`). `StoredDistribution::view()` reports the list back verbatim, so a request that omits `errorCachingMinTTL` on an entry leaves the stored — and later reported — entry with NO `errorCachingMinTTL` at all, although the member's own modelled documentation states the service applies a default of 10 seconds.

That contradicts the rule this very handler follows for every other documented default it meets: `origin.responseTimeout`'s documented 30 seconds and `ipAddressType`'s documented `dualstack` are both materialised a few dozen lines earlier, and the crate's own `health_check_from_request` is documented as "with every member it left out filled in from that member's documented default" for a NESTED structure exactly like this one. The state module even spells out the converse half of the rule for `cacheBehaviorSettings` ("the model documents no default for any member of `CacheSettings`, so nothing is invented for a request that set none") — so where the model DOES document a default, this crate's rule is to record it. `errorCachingMinTTL` is the one documented default left on the floor.

Consequence: `GetDistributions` / `CreateDistribution`'s own answer report a custom error response whose caching window is unspecified, where AWS reports 10.

## Evidence

The model states the default itself — smithy_query(query="shape", shape="com.amazonaws.lightsail#DistributionCustomErrorResponse"), member `errorCachingMinTTL`:
```
"The minimum time, in seconds, that the distribution caches the custom error response before requesting the object again from the origin. If you don't specify a value, the default is 10 seconds."
```

The handler only bounds the list and never looks inside an entry — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_distribution.rs:288-298:
```
        let custom_error_responses = input.custom_error_responses.clone().unwrap_or_default();
        if custom_error_responses.len()
            > crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
        {
            return Err(invalid(format!(
                "customErrorResponses carries {} entries, more than the {} one distribution \
                 holds",
                custom_error_responses.len(),
                crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
            )));
        }
```
and stores them verbatim — create_distribution.rs:427-428:
```
                default_root_object: input.default_root_object.clone(),
                custom_error_responses,
```
reported back verbatim — src/state.rs:12472:
```
            custom_error_responses: Some(self.custom_error_responses.clone()),
```

The SAME handler materialises the other two documented defaults — create_distribution.rs:244-256:
```
        // "The minimum timeout is 1 second, the maximum is 60 seconds, and the
        // default (if you don't specify otherwise) is 30 seconds."
        let (min_timeout, max_timeout) = crate::state::ORIGIN_RESPONSE_TIMEOUT_SECONDS_RANGE;
        let response_timeout = match input.origin.response_timeout {
            None => crate::state::DEFAULT_ORIGIN_RESPONSE_TIMEOUT_SECONDS,
```
and create_distribution.rs:299-304:
```
        // "The IP address type for the distribution. [...] The default value is
        // dualstack."
        let ip_address_type = input
            .ip_address_type
            .clone()
            .unwrap_or_else(|| crate::state::DEFAULT_IP_ADDRESS_TYPE.to_string());
```

The crate states the rule for nested structures — src/state.rs:9874-9876:
```
/// The health check `request` asks for, with every member it left out filled in
/// from that member's documented default, or a refusal naming the member that is
/// outside its documented range.
```
(and fills all six of `ContainerServiceHealthCheckConfig`'s documented defaults at state.rs:9900-9922), while the converse half is stated at src/state.rs:12400-12402:
```
    /// The cache behaviour settings the request carried, or `None` when it
    /// carried none — the model documents no default for any member of
    /// `CacheSettings`, so nothing is invented for a request that set none.
```

Test coverage does NOT pin the defect: every test that sends `customErrorResponses` sends all four members explicitly, so the omitted-member form is never exercised — tests/all/distribution_cdn.rs:335-349 (`json!({"errorCode": .., "responseCode": .., "responsePagePath": page, "errorCachingMinTTL": ttl})`) and tests/all/distribution_update.rs:183-188 (`"errorCachingMinTTL": 10` always sent).

## Steps to Reproduce

1. Against the us-east-1 endpoint, call CreateInstances to create an instance "origin-1" in us-east-1a.
2. Call CreateDistribution with distributionName="cdn-1", bundleId="small_1_0", origin={"name":"origin-1"}, defaultCacheBehavior={"behavior":"cache"} and customErrorResponses=[{"errorCode":404,"responseCode":"404","responsePagePath":"/404.html"}] — note that errorCachingMinTTL is OMITTED.
3. Read the 200 response's `distribution.customErrorResponses[0]`: it carries no `errorCachingMinTTL` member, where the model says the service applies 10.
4. Call GetDistributions and read the same entry — still no `errorCachingMinTTL`.
5. Compare with the same request omitting `origin.responseTimeout` and `ipAddressType`: both come back filled in with their documented defaults (30 and dualstack), from src/ops/create_distribution.rs:244-256 and 299-304.
