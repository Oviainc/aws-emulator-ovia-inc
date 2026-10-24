Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# GetFreeTierUsage accepts a filter Expression that sets more than one root operator

_Filed: 2026-10-24T00:03:52Z_
_Operation: GetFreeTierUsage | Class: validation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The model documents `Expression` as having exactly one root operator: "In the following Contents, you must specify exactly one of the following root operators" (`And`, `Or`, `Not`, `Dimensions`). The emulator instead accepts an expression that sets several of them and silently AND-combines the results (`matches_expression` counts the members only to reject the zero-member case, then intersects every member it found).

Expected: a filter such as `{"Or": [...], "Dimensions": {...}}` or `{"And": [...], "Not": {...}}` is refused with the modelled `ValidationException` (HTTP 400), the same way an expression that sets no root operator is.

Actual: HTTP 200 with a record set computed under an invented conjunction rule, so a client that malforms a filter gets plausible-looking but unverifiable results instead of the service's refusal.

## Evidence

Spec: smithy_query shape=Expression — doc text "In the following Contents, you must specify exactly one of the following root operators." with members Or/And/Not/Dimensions, all optional. Handler: services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs lines 251-298 — each member is evaluated and `matched &=` accumulated; `members == 0` is the only refused arity, so `members >= 2` is accepted. Tests: tests/all/free_tier.rs `an_unevaluable_filter_is_refused` covers an empty expression, a bad dimension key, bad/duplicate MatchOptions and a non-numeric value, but has no case with two root operators set, so no test pins the current behaviour either way.

## Steps to Reproduce

1. Boot the emulator with this service mounted.
2. POST / with X-Amz-Target: AWSFreeTierService.GetFreeTierUsage and body {"maxResults": 1000, "filter": {"Dimensions": {"Key": "SERVICE", "Values": ["AWSLambda"], "MatchOptions": ["EQUALS"]}, "Not": {"Dimensions": {"Key": "REGION", "Values": ["us-east-1"], "MatchOptions": ["EQUALS"]}}}}.
3. Observe HTTP 200 with a filtered record list; expected HTTP 400 ValidationException because the expression sets two root operators.
