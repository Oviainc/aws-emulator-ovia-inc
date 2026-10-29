Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0ebb11a4ec2a284a7bba9ad4aa2d81f032f9138d

# The handler contract docs of GetAutomationRule and ListTagsForResource state a malformed ruleArn is refused 400 InvalidParameterValueException, while src/service.rs answers 403 AccessDeniedException and the suite pins the 403

_Filed: 2026-10-29T14:22:56Z_
_Operation: GetAutomationRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `0ebb11a4ec2a284a7bba9ad4aa2d81f032f9138d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two handler modules document the opposite of what this crate does for a resource ARN that does not satisfy the modelled `RuleArn` form.

`src/ops/get_automation_rule.rs:5-8` tells a reader that such an ARN "is refused at the boundary as `InvalidParameterValueException` (400) — see `src/service.rs`", and `src/ops/list_tags_for_resource.rs:9-12` repeats the same sentence. But `src/service.rs` — the very file both cite — deliberately intercepts those requests BEFORE the modelled-constraint walk and answers `AccessDeniedException` (403), which is what the suite pins for `GetAutomationRule` and for the tagging operations.

THE WIRE BEHAVIOUR IS CORRECT AND MUST NOT CHANGE. The 403 is a deliberate, test-pinned design (authorization is evaluated against the supplied resource string before the service parses it), so the defect is the two stale doc blocks that still describe the pre-`DenyUnauthorizedResource` 400 and hand a reader (or a later fixer) the wrong error contract for these operations. The fix is to correct those two sentences to name `AccessDeniedException` (403) for a string that names no automation rule, and keep the 404 sentence for a well-formed ARN no rule has.

The other four operations in the same gate (`UpdateAutomationRule`, `DeleteAutomationRule`, `TagResource`, `UntagResource`) carry no such claim in their module docs, so these two files are the inconsistent ones.

## Evidence

The handler doc, `src/ops/get_automation_rule.rs:5-8`:
```
//! IMPLEMENTED. Looks the rule up by ARN and reports every field the store holds
//! for it. An ARN no rule has is `ResourceNotFoundException` (404); one that does
//! not satisfy the modelled `RuleArn` pattern is refused at the boundary as
//! `InvalidParameterValueException` (400) — see `src/service.rs`.
```
and the same claim in `src/ops/list_tags_for_resource.rs:9-12`:
```
//! `ResourceNotFoundException` (404); one that does not satisfy the modelled
//! `RuleArn` pattern is refused at the boundary as `InvalidParameterValueException`
//! (400) — see `src/service.rs`.
```

The file they point at says the opposite — `src/service.rs:86-94`:
```
/// Answers one request `AccessDeniedException` (HTTP 403).
///
/// Authorization is evaluated against the resource string a request supplies
/// BEFORE the service parses that string, so a string that identifies no
/// resource of this service is refused as unauthorized rather than as a bad
/// parameter: no permission a caller can hold names a resource the request does
/// not name. The framework's pre-handler validation would otherwise reject the
/// `RuleArn` `@pattern` with a 400, which classifies a permissions failure as a
/// request bug.
```
wired for exactly these two operations among the six, `src/service.rs:77-84` and `src/service.rs:201-210`:
```
const RESOURCE_MEMBERS: [(&str, &str); 6] = [
    ("GetAutomationRule", "ruleArn"),
...
    ("ListTagsForResource", "resourceArn"),
];
```
```
        // Authorization comes before parameter parsing: a request whose
        // resource member does not name an automation rule of this service at
        // all is refused `AccessDeniedException` (403) here, rather than falling
        // to the modelled `RuleArn` `@pattern` rejection (400) that the
        // framework's pre-handler walk would answer with.
        self.names_unauthorized_resource(req)
            .then_some(&self.deny_unauthorized_resource as _)
```

Test coverage pins the 403 for this very operation — `tests/all/automation_rule_lifecycle.rs:1143-1189`:
```
/// A resource string that names no automation rule of this service is refused
/// by AUTHORIZATION (403 `AccessDeniedException`) — authorization is evaluated
/// against the string as supplied, before the service parses it — while a
/// well-formed ARN no rule has gets as far as the lookup and is a 404.
#[tokio::test]
async fn an_arn_is_authorized_on_its_form_then_looked_up() {
...
        ("not-an-arn", "my-rule".to_string(), 403u16),
...
        assert_refused(
            call(&router, "GetAutomationRule", &json!({"ruleArn": rule_arn})).await,
            *expected_status,
            expected_code,
```
and for the tagging path — `tests/all/resource_tags.rs:212` ("refused by authorization with the modelled `AccessDeniedException` (403) —") plus `tests/all/error_payloads.rs:332` ("(`AccessDeniedException`, 403) before the request is parsed, so it never"). So the doc, not the code, is the side that is wrong.

## Steps to Reproduce

1. Read src/ops/get_automation_rule.rs:5-8 and src/ops/list_tags_for_resource.rs:9-12: both state that a ruleArn/resourceArn outside the modelled RuleArn pattern is answered InvalidParameterValueException (400) "at the boundary", citing src/service.rs.
2. Read src/service.rs:86-94 and src/service.rs:197-210: the seam those docs cite refuses exactly that input with AccessDeniedException (403) INSTEAD of the 400, for the six RESOURCE_MEMBERS operations including GetAutomationRule and ListTagsForResource.
3. Run the pinned test tests/all/automation_rule_lifecycle.rs::an_arn_is_authorized_on_its_form_then_looked_up: GetAutomationRule with {"ruleArn": "my-rule"} is asserted 403 AccessDeniedException, not 400.
4. Call GetAutomationRule with {"ruleArn": "my-rule"} against the emulator: 403 AccessDeniedException, contradicting the handler's own contract doc.
5. Fix the two doc blocks to describe the 403 the code answers (and keep the 404 clause for a well-formed ARN no rule has). Do NOT change the response: the 403 is deliberate and test-pinned.
