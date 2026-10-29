Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 67d3ef025eb7c6b921e8b895d06c7f32bf688664

# A multi-violation ValidationException message runs past the 1024 characters the model gives ValidationException$message

_Filed: 2026-10-28T22:40:34Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `67d3ef025eb7c6b921e8b895d06c7f32bf688664`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service words every refused request through one formatter (`crate::service::validation`), whose `envelope` joins ONE clause per violation into a single `message` string with no bound on the resulting length — only on the NUMBER of clauses (`MAX_VIOLATIONS = 100`).

The model constrains that member: `ValidationException$message` targets `BasicString` with `@length(min: 0, max: 1024)`. One clause of the service's own wording is already ~110 characters (`Value at '<path>' failed to satisfy constraint: Member must have length greater than or equal to 1`), so a request carrying about ten rejected members produces a `message` past 1024 characters, and a request carrying the 100 violations the walk hands over produces one around 11,000 characters — ten times the maximum the model states for the member it travels in.

The path is reachable through ordinary modelled input: `TagResource$ResourceTags` is a `ResourceTagList` of `@length(min: 0, max: 200)`, each `ResourceTag$Key` carries `@length(min: 1, max: 128)`, and this crate's own formatter documentation states that the rule "holds for every constrained member of every operation of this service … a member nested in `Rule` or in a `ResourceTag` alike" — so one array of ten tags with an empty `Key` yields ten clauses in one envelope.

The cap that IS stated in the crate bounds the clause COUNT for response-size reasons; nothing in the crate, in a comment or in a test addresses the modelled length of the member the joined text is written into, and no test exercises more than two clauses (the envelope unit test and `tests/all/validation_message_wording.rs` build 1- and 2-clause envelopes, both well under 1024). Each `fieldList` entry's own `message` stays inside the bound — it carries a single clause — so only the envelope diverges.

## Evidence

Smithy, `smithy_query(query="shape", shape="ValidationException")` — the member the envelope is written into:
```
"message": {"target": "BasicString", "length": {"min": 0, "max": 1024}, "pattern": "^[\\s\\S]*$"}
```

The envelope, joined with no length bound — services/service-com-amazonaws-invoicing-invoicing/src/service/validation.rs:151-164:
```
pub(crate) fn envelope(violations: &[Violation]) -> Option<String> {
    let clauses = violations
        .iter()
        .map(clause)
        .collect::<Option<Vec<String>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}
```

The only bound the crate applies — src/service/validation.rs:70-75 (the COUNT, justified by response size, not by the member's modelled length):
```
/// How many violations one refusal states. Each one is rendered TWICE into the body (a
/// clause of `message` and a `fieldList` entry), so an unbounded count turns a small
/// request into an arbitrarily large refusal. The bound is the boundary validator's own
/// (`MAX_VIOLATIONS` in `aws_smithy_schema_server::constraints`), so the two refusal
/// paths read alike.
const MAX_VIOLATIONS: usize = 100;
```
and where it is applied — src/service/validation.rs:176 and :217:
```
    let violations = &violations[..violations.len().min(MAX_VIOLATIONS)];
```
```
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_EXCEPTION.to_string(),
            message: envelope(&violations[..violations.len().min(MAX_VIOLATIONS)])?,
```

One clause's own length — src/service/validation.rs:110-112 and :143-146:
```
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
```
```
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
```

That the walk reports one violation per nested element, in this crate's own words — src/service/validation.rs:35-39:
```
//! Nothing here is keyed to an operation, a member name or a value: the rule is applied
//! to whatever [`Violation`] the walk produces, so it holds for every constrained member
//! of every operation of this service — `InvoiceReceiver`'s `@pattern`, `Name`'s length
//! and pattern, `MaxResults`' range, the ARN patterns, a member nested in `Rule` or in a
//! `ResourceTag` alike.
```

The input that reaches it, `smithy_query(query="shape", shape="ResourceTag,ResourceTagList")`:
```
"ResourceTagList": {"type": "list", "length": {"min": 0, "max": 200}, "member": "ResourceTag"}
"ResourceTag": {"Key": {"target": "ResourceTagKey", "required": true, "length": {"min": 1, "max": 128}}, ...}
```

The refusal is answered for TagResource through the FrontDoor, which serializes whatever the formatter returns with no length check — src/service.rs:305-314:
```
            if let Some(violations) = (entry.violations)(protocol, req) {
                let modelled = validation::field_validation_failed(&violations);
                return protocol
                    .serialize_error(
                        &modelled,
```

Test coverage: the envelope is pinned only for one and two clauses — src/service/validation.rs:304-338 (`the_envelope_counts_the_clauses`, a 1-clause and a 2-clause case) and tests/all/validation_message_wording.rs:226-232, which rebuilds the same `N validation errors detected: …` string from the case's own expected clauses. tests/all/wire_envelope_and_error_detail.rs:470-509 drives a TagResource refusal but with a single missing member, so no existing test reaches a message over 1024 characters.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateInvoiceUnit` and body `{"Name":"alpha","InvoiceReceiver":"111111111111","TaxInheritanceDisabled":false,"Rule":{"LinkedAccounts":["222222222222"]}}` and note the returned `InvoiceUnitArn` (any existing resource ARN works).
3. POST / with `X-Amz-Target: Invoicing.TagResource` and body `{"ResourceArn":"<arn>","ResourceTags":[{"Key":"","Value":"v"}, … ten entries, every `Key` the empty string …]}` — each entry violates `ResourceTagKey`'s `@length(min: 1)`.
4. Observe HTTP 400 `ValidationException` whose `message` is `10 validation errors detected: <clause>; …` — well over 1024 characters, while `smithy_query(query="shape", shape="ValidationException")` gives that member `@length(min: 0, max: 1024)`.
5. Repeat with 100+ offending entries (inside `ResourceTagList`'s `@length(max: 200)`): the message grows to roughly 11,000 characters, since only the clause count is capped (`MAX_VIOLATIONS = 100`) and never the joined length.
6. Note that each `fieldList` entry's own `message` stays inside the bound — only the envelope the model constrains to 1024 exceeds it.
