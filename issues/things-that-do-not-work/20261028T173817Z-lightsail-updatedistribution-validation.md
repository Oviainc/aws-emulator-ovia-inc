Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4a5766ed2caf698065daa3a6a1a461ee27489a64

# A customErrorResponses entry whose responsePagePath does not begin with a forward slash is accepted and stored

_Filed: 2026-10-28T17:38:17Z_
_Operation: UpdateDistribution | Class: validation_

> _Note: this issue was identified against the code base at commit `4a5766ed2caf698065daa3a6a1a461ee27489a64`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DistributionCustomErrorResponse$responsePagePath` is modelled as a bare `string` with no `@pattern`, and its documentation states the rule the model cannot express: the path "must begin with a forward slash (`/`)". `UpdateDistribution` never looks at the member — it checks only how many entries the list carries and then assigns the list onto the stored record — so `responsePagePath: "404.html"`, `responsePagePath: ""` and `responsePagePath: "   "` are all accepted with HTTP 200 and reported back by `GetDistributions` as the distribution's configuration.

This is the one documented-but-unmodelled string rule on this input that the crate does not enforce. Every comparable rule in the crate IS enforced: the handler's own origin-response-timeout range, `health_check_path_value` for a load balancer's ping path, `validate_domain_name` for certificate domains, `is_ipv4_cidr`/`is_ipv6_cidr` for firewall addresses, `is_ec2_instance_type` for a stack's instance type, and `parse_daily_window`/`parse_weekly_window` for database windows — each of them a value the model types as a plain string and the documentation constrains in prose.

## Evidence

The model states the rule — smithy_query(query="shape", shape="com.amazonaws.lightsail#DistributionCustomErrorResponse"), member `responsePagePath`:
```
"The path to the custom error page that the distribution returns to the viewer (for example, /404.html). The path must begin with a forward slash (/) and reference an object that is available from the origin."
```
and the member is an unconstrained string, so the centralized walk cannot refuse it: the same query reports `"responsePagePath": {"target": "string", ...}` with no pattern and no length.

The handler inspects nothing but the list's size — services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/update_distribution.rs:289-300:
```
        if let Some(custom_error_responses) = &input.custom_error_responses {
            if custom_error_responses.len()
                > crate::state::MAX_CUSTOM_ERROR_RESPONSES_PER_DISTRIBUTION
            {
                return Err(invalid(format!(
                    "customErrorResponses carries {} entries, more than the {} one distribution \
                     holds",
```
and writes the entries unexamined — update_distribution.rs:435-437:
```
            if let Some(custom_error_responses) = input.custom_error_responses {
                distribution.custom_error_responses = custom_error_responses;
            }
```
A grep of the whole crate for `response_page_path` finds no reader outside the generated type and the tests (src/ops/types/shared_types.rs:468 `pub response_page_path: Option<String>`), so nothing checks it on any path.

The crate enforces the comparable prose rule on the sibling path member — src/ops/update_load_balancer_attribute.rs:227-234:
```
        crate::state::LOAD_BALANCER_ATTRIBUTE_HEALTH_CHECK_PATH => {
            let Some(path) = crate::state::health_check_path_value(value) else {
                return Err(invalid(format!(
                    "attributeValue '{value}' is not a value of {attribute_name}: it must be \
                     the non-whitespace path to ping on the target"
                )));
```

Test coverage does NOT pin acceptance of a slashless path: every `responsePagePath` any test sends already begins with `/` — tests/all/distribution_update.rs:186 ("/404.html"), 282 ("/denied.html"), 288 ("/busy.html"), 691 (`format!("/e{index}.html")`) and tests/all/distribution_cdn.rs:343/611 (the `page` column, always a leading-slash literal).

## Steps to Reproduce

1. Against the us-east-1 endpoint, create an instance "origin-1" in us-east-1a and a distribution "cdn-1" over it.
2. Call UpdateDistribution with distributionName="cdn-1" and customErrorResponses=[{"errorCode":404,"responseCode":"404","responsePagePath":"404.html","errorCachingMinTTL":10}] — the path carries no leading slash.
3. Observe HTTP 200 instead of the modelled InvalidInputException.
4. Call GetDistributions: customErrorResponses[0].responsePagePath is reported as "404.html", a path the model says "must begin with a forward slash (/)".
5. Repeat with responsePagePath="" and responsePagePath="   " — both are accepted and stored as well.
