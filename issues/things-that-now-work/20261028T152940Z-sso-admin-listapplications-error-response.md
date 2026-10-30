Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: ea262c1f0c770608da2cb6c8011fa8bda70e1973

# ListApplications reports an InstanceArn this account holds no instance for as a 400 ValidationException, while its two sibling enumerations with the identical modelled error set answer 403 AccessDeniedException

_Filed: 2026-10-28T15:29:40Z_
_Operation: ListApplications | Class: error-response_

> _Note: this issue was identified against the code base at commit `ea262c1f0c770608da2cb6c8011fa8bda70e1973`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three instance-scoped enumerations of this service model the SAME error set — `AccessDeniedException` (403), `InternalServerException` (500), `ThrottlingException` (429), `ValidationException` (400), and NO `ResourceNotFoundException`: `ListApplications`, `ListRegions` and `ListTrustedTokenIssuers`. All three resolve the request's required `InstanceArn` and refuse when this account holds no instance under it, and each handler states a crate-wide rule for which channel that refusal goes on. The two statements contradict each other, and `ListApplications` is the outlier:

* `ListRegions` answers `AccessDeniedException` and states the rule as the crate's: "`AccessDeniedException` is the not-found channel the instance-scoped reads of this service declare".
* `ListTrustedTokenIssuers` answers `AccessDeniedException` too, for the same stated reason, naming `CreateTrustedTokenIssuer` as the handler it agrees with.
* `DescribeInstance` and `CreateTrustedTokenIssuer` — the other two operations of this crate that resolve an `InstanceArn` and model no `ResourceNotFoundException` — also answer `AccessDeniedException` for exactly this condition.
* `ListApplications` alone answers `ValidationException`, claiming "This operation models no `ResourceNotFoundException`, so the refusal goes on the client-input channel it does model" — although the model gives it the very `AccessDeniedException` its four siblings use for the same condition.

Consequence: one condition (a syntactically valid `InstanceArn` that resolves to no instance) is reported with two different HTTP statuses and two different error codes depending on which enumeration a client called — 403 `AccessDeniedException` from `ListRegions`/`ListTrustedTokenIssuers`/`DescribeInstance`, 400 `ValidationException` from `ListApplications`. A client that branches on the error code (the normal SDK behaviour: retry/credential handling for `AccessDeniedException` vs. request-fixing for `ValidationException`) classifies the same mistake two ways. One of the two rule statements is wrong; they cannot both be the crate's rule.

## Evidence

Model (smithy_query query="operation"): `ListApplications` errors are exactly `[AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400]` — the SAME set as `ListRegions` and `ListTrustedTokenIssuers`; none of the three declares `ResourceNotFoundException`, and all three declare `AccessDeniedException`.

The outlier — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_applications.rs:224-238:
```
            // Applications are enumerated UNDER an instance, so an `InstanceArn` this
            // account holds no instance for is refused rather than answered with an empty
            // page that would read as "this instance has no applications". This operation
            // models no `ResourceNotFoundException`, so the refusal goes on the
            // client-input channel it does model, naming the ARN that resolved to nothing.
            if store.instance(&input.instance_arn).is_none() {
                return Err(ListApplicationsErrorModeled::ValidationException(
                    ValidationException {
                        message: Some(crate::state::instance_not_found_message(
                            &input.instance_arn,
                        )),
```

The rule it breaks, stated as the crate's — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_regions.rs:190-201:
```
            // An `InstanceArn` this account holds no instance for: this operation models no
            // `ResourceNotFoundException`, and `AccessDeniedException` is the not-found
            // channel the instance-scoped reads of this service declare.
            let Some(instance) = store.instance(&input.instance_arn) else {
                return Err(ListRegionsErrorModeled::AccessDeniedException(
                    AccessDeniedException {
                        message: Some(crate::state::instance_not_found_message(
                            &input.instance_arn,
                        )),
```

The same choice again — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/list_trusted_token_issuers.rs:203-220:
```
            // Issuers are enumerated UNDER an instance, so an `InstanceArn` this account
            // holds no instance for is refused rather than answered with an empty page
            // that would read as "this instance has no trusted token issuers". This
            // operation models no `ResourceNotFoundException`, and an instance this
            // account does not hold is one the caller may not act on, so the refusal goes
            // on the modelled `AccessDeniedException` channel — the same channel, with the
            // same wording, `CreateTrustedTokenIssuer` answers an unheld `InstanceArn`
            // with.
            if store.instance(&input.instance_arn).is_none() {
                return Err(ListTrustedTokenIssuersErrorModeled::AccessDeniedException(
```

and services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_instance.rs:213-222:
```
        let Some(instance) = guard.instance(&input.instance_arn) else {
            return Err(DescribeInstanceErrorModeled::AccessDeniedException(
```

Test coverage: services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/unknown_instance_message.rs drives `ListApplications`, `ListRegions`, `ListTrustedTokenIssuers` and `DescribeInstance` over the same unknown ARN (lines 94-162) but pins only the detail TEXT, explicitly tolerating any of the three codes — lines 234-242:
```
    assert!(
        matches!(
            code.as_str(),
            "ResourceNotFoundException" | "AccessDeniedException" | "ValidationException"
        ),
        "case {:?}: a missing instance is reported on a channel the operation MODELS, got \
         {code} {body}",
```
so no test states a reason for the 400, and aligning the channel would leave every existing assertion passing.

## Steps to Reproduce

1. Start the emulator and POST / with `X-Amz-Target: SWBExternalService.ListInstances` to see the account's instance; pick an `InstanceArn` the account does NOT hold, e.g. `arn:aws:sso:::instance/ssoins-1111111111111111`.
2. POST / with `X-Amz-Target: SWBExternalService.ListRegions` and `{"InstanceArn":"arn:aws:sso:::instance/ssoins-1111111111111111"}` — 403 `AccessDeniedException` with the crate's single instance-not-found text.
3. POST / with `X-Amz-Target: SWBExternalService.ListTrustedTokenIssuers` and the same body — 403 `AccessDeniedException`, same text.
4. POST / with `X-Amz-Target: SWBExternalService.DescribeInstance` and the same body — 403 `AccessDeniedException`, same text.
5. POST / with `X-Amz-Target: SWBExternalService.ListApplications` and the same body — 400 `ValidationException` instead, although this operation models the same `AccessDeniedException` its three siblings answered on and models no `ResourceNotFoundException` either.
