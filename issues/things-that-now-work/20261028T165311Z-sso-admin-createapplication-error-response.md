Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 78c035fb932ec52ae00c0e559fb4c57d057723ed

# CreateApplication reports an unknown ApplicationProviderArn with a sentence of its own, although its comment claims it answers "the same ResourceNotFoundException DescribeApplicationProvider answers an unknown provider ARN with"

_Filed: 2026-10-28T16:53:11Z_
_Operation: CreateApplication | Class: error-response_

> _Note: this issue was identified against the code base at commit `78c035fb932ec52ae00c0e559fb4c57d057723ed`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateApplication` resolves the request's `ApplicationProviderArn` against the same fixed catalogue `DescribeApplicationProvider` serves (both go through `crate::state::built_in_application_provider`). For the one condition "this ARN names no published provider", the two handlers build two different detail texts inline:

* `CreateApplication`: `"Could not find ApplicationProvider with arn {}."`
* `DescribeApplicationProvider`: `"No application provider with arn '{}' was found"`

The comment directly above the `CreateApplication` refusal states the opposite — that this is "the same `ResourceNotFoundException` `DescribeApplicationProvider` answers an unknown provider ARN with". `Message` is the only member these error shapes carry (see `src/service/refusal.rs`'s `DETAIL_MEMBER`), so "the same `ResourceNotFoundException`" is a claim about the only thing a client can read, and the code under the comment does not make good on it.

The crate states the one-condition-one-sentence rule explicitly for the other ARN it resolves in many operations — `instance_not_found_message` is a single helper precisely "so a client that branches on the detail text — the only member these error shapes carry — sees the same sentence for one ARN whichever operation it called". There is no `application_provider_not_found_message` helper, so the provider ARN is the one resolved identifier of this crate whose not-found text is hand-written per handler and differs between them.

No test pins either sentence: both suites assert only the error CODE, and the create suite additionally asserts only that the message CONTAINS the ARN — so single-homing the text leaves every existing assertion passing.

## Evidence

The comment and the code it sits over — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_application.rs:259-275:
```
        // "The ARN of the application provider under which the operation will run": an
        // application provider is AWS-owned, so the only ones that exist are the ones this
        // service publishes, and an ARN naming no provider of that catalog resolves to
        // nothing — the same `ResourceNotFoundException` `DescribeApplicationProvider`
        // answers an unknown provider ARN with.
        let provider = crate::state::built_in_application_provider(
            &input.application_provider_arn,
        )
        .ok_or_else(|| {
            CreateApplicationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(format!(
                    "Could not find ApplicationProvider with arn {}.",
                    input.application_provider_arn
                )),
```

The other side — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/describe_application_provider.rs:172-181:
```
        let Some(provider) =
            crate::state::built_in_application_provider(&input.application_provider_arn)
        else {
            return Err(
                DescribeApplicationProviderErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "No application provider with arn '{}' was found",
                            input.application_provider_arn
                        )),
```

The crate's own rule for a resolved identifier's not-found text — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/state.rs:252-260:
```
/// One condition reads ONE way: every operation that resolves an `InstanceArn` reports it
/// through this helper, so a client that branches on the detail text — the only member
/// these error shapes carry — sees the same sentence for one ARN whichever operation it
/// called.
///
/// Built from the request: it names the ARN that resolved to nothing.
pub fn instance_not_found_message(instance_arn: &str) -> String {
    format!("No IAM Identity Center instance {instance_arn} exists in this account")
```
and the only member a client can read it in — services/service-com-amazonaws-ssoadmin-swbexternalservice/src/service/refusal.rs:70-71:
```
/// The body member this service reports an exception's detail text in.
pub const DETAIL_MEMBER: &str = "Message";
```

A grep for `application_provider` over `src/` shows no shared not-found helper (only `application_provider_unsupported_message` at state.rs:439), so the two sentences above are the only two definitions of this one condition.

Test coverage: services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/application_create_providers.rs:173-175 asserts only `message.contains(&arn)`, and tests/all/application_provider_describe.rs:211-212 asserts only the code `"ResourceNotFoundException"` — neither pins a sentence, so unifying the wording changes no test outcome.

## Steps to Reproduce

1. Start the emulator and ListInstances to capture the account's InstanceArn.
2. POST / with X-Amz-Target: SWBExternalService.DescribeApplicationProvider and {"ApplicationProviderArn":"arn:aws:sso::aws:applicationProvider/no-such-provider"} — 404 with Message "No application provider with arn 'arn:aws:sso::aws:applicationProvider/no-such-provider' was found".
3. POST / with X-Amz-Target: SWBExternalService.CreateApplication and {"InstanceArn":"<arn>","ApplicationProviderArn":"arn:aws:sso::aws:applicationProvider/no-such-provider","Name":"Demo"} — 404 with Message "Could not find ApplicationProvider with arn arn:aws:sso::aws:applicationProvider/no-such-provider." instead.
4. Compare the two Message values: one condition, two sentences, against the comment at create_application.rs:262-263 that says this is "the same ResourceNotFoundException DescribeApplicationProvider answers an unknown provider ARN with".
