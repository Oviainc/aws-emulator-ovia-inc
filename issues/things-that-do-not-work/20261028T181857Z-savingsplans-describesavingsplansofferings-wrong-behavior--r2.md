Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 8aa1bc3970afbc278ab25e0eb153e57974fffad6

# EC2 Instance offerings report a serviceCode ("AmazonEC2") outside the member's modeled pattern, so they can never be found by the serviceCodes filter

_Filed: 2026-10-28T18:18:57Z_
_Operation: DescribeSavingsPlansOfferings | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `8aa1bc3970afbc278ab25e0eb153e57974fffad6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The seeded offering catalog gives every `EC2Instance` offering the service code `"AmazonEC2"`, and `DescribeSavingsPlansOfferings` projects that string straight into `SavingsPlanOffering$serviceCode`.

That member targets `com.amazonaws.savingsplans#SavingsPlanServiceCode`, whose `@pattern` is `^[a-zA-Z]+$` — letters only, no digits. `"AmazonEC2"` ends in a digit, so the value the handler emits cannot satisfy the constraint declared on the member it is emitted into.

The same shape is the member type of the operation's own `serviceCodes` request filter (`SavingsPlanServiceCodeList` -> `SavingsPlanServiceCode`, same `^[a-zA-Z]+$`). The crate's `WithSubmittedValues` wrapper runs the framework's constraint walk over the built input before the handler is reached (src/service.rs:213-226), so a request carrying `serviceCodes: ["AmazonEC2"]` is refused with a 400 `ValidationException` before `admits` ever runs. The result is that the emulator publishes a `serviceCode` value that its own input validator rejects: of the 2,646 seeded offerings, the 2,376 `EC2Instance` rows (2 terms x 3 payment options x 22 regions x 18 instance families) are unreachable through the `serviceCodes` filter, while the other three plan types round-trip fine because their codes are letters-only (`ComputeSavingsPlans`, `AmazonSageMaker`, `DatabaseSavingsPlans`).

This is a gap in the crate's own discipline, not a new feature: the catalog builder audits every other projected string against its shape's pattern and says so in comments (`SavingsPlanUsageType`, `SavingsPlanDescription`), and `tests/all/offering_catalog.rs:154-163` asserts the emitted `usageType` satisfies the model's constraint. `serviceCode` is the one projected string with no such audit, and it is the one that breaks.

Note the asymmetry that hides the bug: `SavingsPlanRate$serviceCode` / `DescribeSavingsPlansOfferingRates$serviceCodes` target the `SavingsPlanRateServiceCode` ENUM, which does list `AmazonEC2`, so `tests/all/offering_rate_catalog.rs:662` sends `serviceCodes: ["AmazonEC2"]` successfully against the RATE search. Only the OFFERING search's pattern-constrained shape is violated, and no test exercises it with that value.

## Evidence

Smithy — the constraint on the member the handler fills, and on the filter that is supposed to accept it (smithy_query shape):

```
"SavingsPlanServiceCode": {"shape_id": "com.amazonaws.savingsplans#SavingsPlanServiceCode", "type": "string", "length": {"min": 0, "max": 255}, "pattern": "^[a-zA-Z]+$"}
"SavingsPlanOffering": { ... "serviceCode": {"target": "SavingsPlanServiceCode", "length": {"min": 0, "max": 255}, "pattern": "^[a-zA-Z]+$"} ... }
"SavingsPlanServiceCodeList": {"type": "list", "member": {"target": "SavingsPlanServiceCode", "length": {"min": 0, "max": 255}, "pattern": "^[a-zA-Z]+$"}}
"DescribeSavingsPlansOfferingsRequest": { ... "serviceCodes": {"target": "SavingsPlanServiceCodeList", "type": "list"} ... }
```

The code that emits a value with a digit in it — src/state.rs:963-985 (the `EC2Instance` arm of `seed_offerings`), the 7th positional argument of `offering(..)` being `service_code`:

```
    for (duration_seconds, term_label, term_token) in TERMS {
        for payment_option in PAYMENT_OPTIONS {
            for (region, currency) in REGIONS {
                for (family, _) in EC2_INSTANCE_FAMILIES {
                    offerings.push(offering(
                        "EC2Instance",
                        &EC2_INSTANCE_PRODUCT_TYPES,
                        &offering_description(term_label, payment_option, "EC2 Instance"),
                        payment_option,
                        duration_seconds,
                        currency,
                        "AmazonEC2",                      // <-- src/state.rs:974
```

Projected onto the wire unchanged — src/ops/describe_savings_plans_offerings.rs:268-279:

```
fn project(offering: &crate::state::Offering) -> SavingsPlanOffering {
    SavingsPlanOffering {
        ...
        service_code: Some(offering.service_code.clone()),   // <-- :276
```

And matched against the request filter by exact string equality — src/ops/describe_savings_plans_offerings.rs:235:

```
        || !filter_admits(input.service_codes.as_ref(), &offering.service_code)
```

The input is pattern-checked before the handler runs, so `serviceCodes: ["AmazonEC2"]` never reaches that line — src/service.rs:213-226:

```
            let refused = service_common::validation::refusing_violations::<O>(
                svc.definition().protocol(),
                req,
            );
            let stated = rooted_at_request(&refused);
            if let Some(message) = envelope(&stated) {
                return svc.definition().refusal_response(
                    ProtocolError::Invalid(stated),
```

The crate's own precedent for auditing a projected string against its shape's pattern — src/state.rs:798-801:

```
/// `family` carries the region prefix and any further dimension the plan type is
/// scoped to (an EC2 instance family). `SavingsPlanUsageType` admits `:`, `.`,
/// `/` and `-` (pattern `^[a-zA-Z0-9_ \/.:-]+$`), so the `:`-separated form is
/// inside the model's constraint.
```

and the test that enforces it for `usageType` — tests/all/offering_catalog.rs:154-163:

```
        // SavingsPlanUsageType: length 0..255, pattern ^[a-zA-Z0-9_ \/.:-]+$.
        assert!(
            !usage_type.is_empty()
                && usage_type.len() <= 255
                && usage_type
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric()
                        || matches!(b, b'_' | b' ' | b'/' | b'.' | b':' | b'-')),
            "case {id:?}: usageType {usage_type:?} must satisfy the model's constraint"
        );
```

Test coverage: the `serviceCodes` round-trip is pinned only for a letters-only code — tests/all/savings_plans_lifecycle.rs:299-303:

```
        (
            "serviceCodes",
            json!({ "serviceCodes": ["AmazonSageMaker"], "maxResults": 1000 }),
            Box::new(|o: &Value| text(o, "serviceCode") == "AmazonSageMaker"),
        ),
```

and tests/all/offering_catalog.rs:474-481 only uses `"serviceCodes": ["AmazonSageMaker"]` as a deliberately-empty case. No test sends `serviceCodes: ["AmazonEC2"]` to `DescribeSavingsPlansOfferings`; the one place that string appears in a request is tests/all/offering_rate_catalog.rs:662, against `DescribeSavingsPlansOfferingRates`, whose `serviceCodes` target is the `SavingsPlanRateServiceCode` enum (which does contain `AmazonEC2`), so that row passes and the offering-search violation stays invisible.

## Steps to Reproduce

1. POST /DescribeSavingsPlansOfferings with `{"planTypes": ["EC2Instance"], "maxResults": 5}` and read `searchResults[0].serviceCode`: it is `"AmazonEC2"`.
2. Validate that response member against the model: `SavingsPlanOffering$serviceCode` targets `SavingsPlanServiceCode`, `@pattern` `^[a-zA-Z]+$`. `"AmazonEC2"` contains the digit `2`, so the emitted value is outside the constraint declared on the member.
3. Feed that value back into the operation's own filter: POST /DescribeSavingsPlansOfferings with `{"serviceCodes": ["AmazonEC2"], "maxResults": 5}`. The framework's constraint walk in src/service.rs:213-226 refuses it with HTTP 400 ValidationException before the handler runs, so the 2,376 EC2 Instance offerings cannot be searched by the serviceCode they themselves report.
4. Contrast with the sibling plan types: `{"serviceCodes": ["AmazonSageMaker"]}` (tests/all/savings_plans_lifecycle.rs:299-303) and `{"serviceCodes": ["ComputeSavingsPlans"]}` are accepted and return the offerings that report them, because those codes are letters-only.
5. Contrast with the rate search, which uses the enum-typed shape: POST /DescribeSavingsPlansOfferingRates with `{"products": ["Lambda"], "serviceCodes": ["AmazonEC2"], "maxResults": 50}` (tests/all/offering_rate_catalog.rs:662) is accepted — confirming only the pattern-constrained offering-search shape is violated.
