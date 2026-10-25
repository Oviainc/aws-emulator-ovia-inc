Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# ServiceQuotaExceededException reports ServiceCode "bcm-data-exports" where the model documents a fixed "AWSBillingAndCostManagementDataExports"

_Filed: 2026-10-24T22:59:51Z_
_Operation: CreateExport | Class: error-response_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateExport's modelled `ServiceQuotaExceededException` carries a `ServiceCode` member whose own documentation states the value it always takes. The handler instead fills it with this service's SigV4/endpoint-prefix name, so the quota refusal a client receives names a service code the model says it can never carry.

`crate::state::SERVICE_CODE` is defined as an alias of `SIGNING_NAME` (`"bcm-data-exports"`), and the comment above it states the rule the crate applied ("this service's endpoint prefix") — which is a different rule from the one the model's member documentation states. The `QuotaCode` sibling on the same error is an emulator choice the model leaves open, but `ServiceCode` is not: the model fixes its value in prose.

This is the only place in the crate that emits `ServiceCode`, and the single existing test for the quota path asserts only `QuotaCode`, so nothing in the crate justifies or pins the divergence.

## Evidence

Smithy model (smithy_query query="doc" shape="ServiceQuotaExceededException" member="ServiceCode"):
  "The service code that exceeded quota. It will always be “AWSBillingAndCostManagementDataExports”."
(`ServiceCode` is `@required` on the shape: smithy_query shape="ServiceQuotaExceededException" -> `"ServiceCode": {"target": "GenericString", "required": true, ...}`.)

Handler, services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/ops/create_export.rs:286-287:
```
                    quota_code: crate::state::EXPORTS_PER_ACCOUNT_QUOTA_CODE.to_string(),
                    service_code: crate::state::SERVICE_CODE.to_string(),
```

The value it resolves to, services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/src/state.rs:128-129:
```
/// The `ServiceCode` a quota failure reports: this service's endpoint prefix.
pub const SERVICE_CODE: &str = SIGNING_NAME;
```
and state.rs:92:
```
pub const SIGNING_NAME: &str = "bcm-data-exports";
```

Test coverage: the one test over this path, services/service-com-amazonaws-bcmdataexports-awsbillingandcostmanagementdataexports/tests/all/export_lifecycle.rs:1353 `creating_past_the_export_limit_is_refused_with_the_quota_error`, asserts the status/code pair and `body["QuotaCode"] == json!("ExportsPerAccount")` (lines 1370-1379) and never inspects `ServiceCode` — so no test pins the current value and nothing in the crate states a reason to diverge from the modelled sentence.

## Steps to Reproduce

1. Boot the emulator and call CreateExport MAX_EXPORTS (1000) times with distinct Export.Name values so the store reaches its bound.
2. Call CreateExport once more with a fresh name.
3. Observe the HTTP 402 ServiceQuotaExceededException body: `ServiceCode` is "bcm-data-exports".
4. Compare with the modelled member documentation for ServiceQuotaExceededException$ServiceCode, which states the value "will always be “AWSBillingAndCostManagementDataExports”".
