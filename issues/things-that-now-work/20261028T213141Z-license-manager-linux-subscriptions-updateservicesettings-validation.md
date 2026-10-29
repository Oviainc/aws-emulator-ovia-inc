Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd

# The home Region is recorded without the Region-catalog check the same handler applies to SourceRegions

_Filed: 2026-10-28T21:31:41Z_
_Operation: UpdateServiceSettings | Class: validation_

> _Note: this issue was identified against the code base at commit `83b2b15993ea2e3b2b9e4b142ed7a7a8695c4dfd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateServiceSettings` validates every `SourceRegions` entry against the shared Region catalog (`crate::state::is_known_region`) and refuses the request with the modelled `ValidationException` when an entry does not name an AWS Region. In the very same handler, a few lines further down, the HOME Region is taken from the request's SigV4 credential scope through `crate::state::request_region` and pushed into `home_regions` with NO such check — `request_region` only filters on the ARN grammar (`[a-z0-9-.]{1,63}`), which any invented Region code satisfies.

Consequences, all from the quoted code:

1. A request signed for a credential scope naming a non-existent Region (e.g. `us-east-99`) is answered 200 and the bogus value is persisted into the settings singleton's `home_regions`, so `GetServiceSettings` and every later `UpdateServiceSettings` report `HomeRegions: ["us-east-99"]` — a value the SAME call would have refused with `Invalid region name/format : us-east-99` had the caller put it in `SourceRegions` instead.
2. Nothing in the API ever removes a home Region, so that entry permanently occupies the account's home-Region set. A subsequent, legitimate `UpdateServiceSettings` signed for a real Region is then refused with `Linux subscription data is already aggregated in home Region(s) us-east-99; set AllowUpdate to true …` — a refusal produced entirely by a value the handler's own rule says may not name a Region.

The crate already owns the helper and states the rule the home-Region path skips, so this is a path that does not call a check its sibling path in the same handler does.

## Evidence

The check the SourceRegions path applies — `src/ops/update_service_settings.rs:173-185`:
```
        // Input validation first, so a refused request leaves the account's settings
        // exactly as they were. `SourceRegions` is a plain `StringList` in the model, so
        // nothing in the request walk checks that its entries name Regions; this
        // operation does, against the published Region codes
        // (`crate::state::is_known_region`).
        if let Some(unknown) = input
            .linux_subscriptions_discovery_settings
            .source_regions
            .iter()
            .find(|source| !crate::state::is_known_region(source))
        {
            return Err(validation(format!("Invalid region name/format : {unknown}")));
        }
```

The home-Region path, in the same handler, with no such check — `src/ops/update_service_settings.rs:187-209` (the value comes from line 170 `let region = crate::state::request_region(&ctx.parts().headers);`):
```
        let mut home_regions = state
            .service_settings
            .as_ref()
            .map(|settings| settings.home_regions.clone())
            .unwrap_or_default();
        if !home_regions.contains(&region) {
            // The first update makes the Region it was signed for the home Region;
            ...
            home_regions.push(region);
        }
```

What `request_region` actually guarantees — `src/state.rs:796-812`:
```
/// The region this request was signed for, or the emulator's default when the request
/// carries no usable SigV4 credential scope.
///
/// Values outside the ARN grammar's region segment (`[a-z0-9-.]{1,63}`) are ignored, so
/// a minted ARN always satisfies the modelled `SubscriptionProviderArn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
```
— i.e. the ARN grammar only; `us-east-99` passes it.

The rule the crate states for a Region-valued `StringList` — `src/state.rs:814-826`:
```
/// Whether `region` names an AWS Region.
///
/// The catalog is `emulator_telemetry::policy::KNOWN_REGIONS` … this
/// service models no Region enum — `SourceRegions` is a plain `StringList` — so the set
/// a value may name is exactly that shared catalog, and it must not drift from it.
pub fn is_known_region(region: &str) -> bool {
```
`HomeRegions` is the same `StringList` shape (`smithy_query(query="operation",operation="UpdateServiceSettings")`: `"HomeRegions": {"target": "StringList", "length": {"min": 1, "max": 100}, "uniqueItems": true}`), documented as "The Region in which License Manager displays the aggregated data for Linux subscriptions."

The signed Region is fully caller-controlled and unchecked at the boundary — `tests/all/harness.rs:198-208` builds the credential scope from an arbitrary `region` string:
```
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTEXAMPLE/20260101/{region}/{SIGNING_NAME}\
                 /aws4_request, SignedHeaders=host, Signature=0"
```

Test coverage: `tests/all/error_taxonomy.rs:127-184` (`a_source_region_outside_the_region_catalog_is_refused`) pins the catalog rule for `SourceRegions` only — including near-misses such as `"us-west-3"`, `"eu-fake-1"`, `"us-east"`. `tests/all/subscription_providers.rs:328-360` (`a_second_home_region_needs_allow_update`) and `tests/all/provider_status_transition.rs:277-299` only ever sign for real Regions (`us-west-2`, `eu-west-1`, `ap-southeast-7`, `us-gov-east-1`), so no test pins the acceptance of an unknown signed Region as a home Region.

## Steps to Reproduce

1. Boot the emulator and send `POST /subscription/UpdateServiceSettings` signed for a credential scope naming a non-existent Region, e.g. `Credential=AKIDTESTEXAMPLE/20260101/us-east-99/license-manager-linux-subscriptions/aws4_request`, with body `{"LinuxSubscriptionsDiscovery":"Enabled","LinuxSubscriptionsDiscoverySettings":{"SourceRegions":["us-east-1"],"OrganizationIntegration":"Disabled"}}`.
2. Observe HTTP 200 with `"HomeRegions": ["us-east-99"]`, and `POST /subscription/GetServiceSettings` reporting the same.
3. Send the identical request but with `"SourceRegions":["us-east-99"]` while signed for a real Region: it is refused with 400 `ValidationException` / `Invalid region name/format : us-east-99` — the same value, refused on one path and persisted on the other.
4. Now send `POST /subscription/UpdateServiceSettings` signed for a real Region (e.g. `us-west-2`) without `AllowUpdate`: it is refused with `Linux subscription data is already aggregated in home Region(s) us-east-99; set AllowUpdate to true …`, i.e. the bogus home Region permanently blocks the single-home-Region path.
