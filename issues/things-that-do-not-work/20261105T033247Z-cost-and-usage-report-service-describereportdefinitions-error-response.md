Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: f788da56fca8042d5f8354f4ce9bfd6169341434

# Invalid-NextToken refusal uses the synthetic framework ValidationException instead of this service's modelled one

_Filed: 2026-11-05T03:32:47Z_
_Operation: DescribeReportDefinitions | Class: error-response_

> _Note: this issue was identified against the code base at commit `f788da56fca8042d5f8354f4ce9bfd6169341434`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The invalid-page-token refusal is built with `crate::scaffolded::Scaffolded::validation(..)`, which serializes the SYNTHETIC `smithy.framework#ValidationException` — a shape the crate's own module doc says is "NOT in any service's model". This service DOES model its own `ValidationException` (`com.amazonaws.costandusagereportservice#ValidationException`, generated at src/ops/types/shared_types.rs:20-24), and every other refusal in this crate returns that modelled shape.

The crate's own instructions in src/scaffolded.rs say to do the opposite of what this handler does, naming even the mechanism for the case where the SERVICE models the error but the OPERATION does not list it (`#[operation_error]`'s `extra_errors`) — which is exactly `DescribeReportDefinitions`' situation.

The observable divergence is the body: the synthetic shape declares its detail member as `message` (src/scaffolded.rs:117-128), while the modelled shape's member is `Message`, so this one operation's refusal detail can land under a different key from every other refusal of the same service. The error code (`__type`) and the 400 status are the same either way; what differs is the shape and its member. The crate's test helpers sidestep the difference by looking both spellings up, so nothing pins the current spelling.

## Evidence

This crate's own rule, src/scaffolded.rs:215-220:
```
    /// If this service's model declares its own `ValidationException` (or another
    /// client-input error) use THAT: the modeled shape may carry members beyond
    /// `message` — a `reason`, a field list — and a real client reads them. When the
    /// operation already lists it, it is a plain variant on the generated error enum;
    /// `#[operation_error]`'s `extra_errors` is only for widening in a shape the
    /// SERVICE models that this OPERATION does not list.
```

What the handler does instead, src/ops/describe_report_definitions.rs:220-225:
```
    let invalid = || {
        crate::scaffolded::Scaffolded::validation(
            "The NextToken in the request is not valid. Pass the NextToken a previous \
             DescribeReportDefinitions response returned.",
        )
    };
```

The shape that reaches the wire, src/scaffolded.rs:130-144: "In the `smithy.framework` namespace for the same reason as `NotImplemented`: the shape is not in any service's model" — with its member declared lowercase at src/scaffolded.rs:117-127 (`ShapeId::from_static("smithy.framework#ValidationException$message", ..., "message", 0)`).

The service DOES model it — smithy_query(shape="ValidationException"): `com.amazonaws.costandusagereportservice#ValidationException`, `"members": {"Message": {"target": "ErrorMessage"}}` — and sibling handlers return that modelled shape: src/ops/modify_report_definition.rs:186-191 (`ModifyReportDefinitionErrorModeled::ValidationException(ValidationException { message: Some(..) })`), src/ops/put_report_definition.rs:193-198, src/ops/tag_resource.rs:168-176.

No test pins the current spelling — tests/all/refusal_envelope.rs:35-44 accepts either: `["Message", "message"].into_iter().find_map(|key| body.get(key))`; tests/all/report_definition_lifecycle.rs's `describe_refuses_a_next_token_it_did_not_issue` asserts only the 400 and the error code.

## Steps to Reproduce

1. Call `DescribeReportDefinitions` with `{"NextToken": "zzz"}` (in-pattern but not a token the service issued).
2. Observe the 400 body: it is the synthetic `smithy.framework#ValidationException` with its detail under the synthetic schema's `message` member, not this service's modelled `com.amazonaws.costandusagereportservice#ValidationException`.
3. Compare with the 400 body of `ModifyReportDefinition` on a name the account does not hold (src/ops/modify_report_definition.rs:160-163), which returns the modelled shape — two different shapes for one logical refusal inside one service.
