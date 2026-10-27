Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 83df1cdc32a5bdf510e34eb14380f1c3f1e573c9

# Tag validation report can exceed the modelled 2048-character ErrorMessage bound

_Filed: 2026-10-27T14:54:27Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `83df1cdc32a5bdf510e34eb14380f1c3f1e573c9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every modelled error of this service carries exactly one member, `Message`, targeting `ErrorMessage`, which the model bounds at `@length(min: 1, max: 2048)`. The tag-character rule this crate enforces in a handler builds its `ValidationException` message with ONE clause per offending tag key and ONE per offending tag value, and `validation_report` then joins ALL of them into a single string with no bound and no eliding. `Tags` is modelled `@length 0..200`, so one well-formed `TagResource` request can produce up to 400 clauses of ~150 characters each — roughly 60,000 characters in the `Message` member the model caps at 2048. The bound is already exceeded by a request carrying only 14 tags whose key and value are both outside the documented character set (28 clauses x ~150 = ~4,250 characters).

The same unbounded report reaches the wire through `CreateRescoreExecutionPlan` (which calls the same `tag_set_rejection`), through `UntagResource` (`tag_key_rejection`, up to 200 clauses for a `TagKeys` list the model also bounds at 200), and through `service.rs`'s `override_refusal`, which words a boundary refusal with `state::refusal_report(violations)` — the same `validation_report` join. Nothing in the crate elides, truncates or caps the report: the only length statement anywhere near it is the vendor's own note that a caller-controlled interpolation "would amplify a request into an arbitrarily long response", which is why the framework pre-elides a violation's *value* — the clause COUNT here is caller-controlled in exactly the same way and is not bounded.

The crate's comment above `tag_set_rejection` explains why every offending position is reported ("so a caller is told about all of them at once"), but says nothing about the modelled length of the member the report has to fit in, so the divergence from the model's 2048 is unjustified rather than deliberate.

## Evidence

Model (smithy_query query="shape" shape="ValidationException"):
  "ValidationException": { "members": { "Message": { "target": "ErrorMessage", "length": {"min": 1, "max": 2048}, "pattern": "^\\P{C}*$" } } }
(the same `ErrorMessage` target and bound on `ResourceUnavailableException$Message`, `ConflictException$Message`, `ResourceNotFoundException$Message`, `ServiceQuotaExceededException$Message`), and `TagResource` input `"Tags": {"target": "TagList", "required": true, "length": {"min": 0, "max": 200}}`.

Code — one clause per offending key AND value, with no bound on how many:
`services/.../src/state.rs:579-606`:
    pub fn tag_set_rejection(list_member: &str, tags: &[Tag]) -> Option<ValidationException> {
        let mut clauses = Vec::new();
        for (index, tag) in tags.iter().enumerate() {
            ... clauses.push(clause(&element_path(list_member, position, Some("Key")), &tag_character_requirement())); ...
            if !tag_text_is_permitted(&tag.value) { clauses.push(clause(... Some("Value") ...)); }
        }
        (!clauses.is_empty()).then(|| ValidationException { message: Some(validation_report(&clauses)) })
    }

`services/.../src/state.rs:213-222` joins every clause into the one `Message`:
    pub fn validation_report(clauses: &[String]) -> String {
        match clauses { [single] => format!("1 validation error detected: {single}"),
            many => format!("{} validation errors detected: {}", many.len(), many.join("; ")) } }

`services/.../src/state.rs:549-555` — each clause carries the full ~91-character requirement:
    format!("Member must consist of Unicode letters, digits, white space and the symbols {}", symbols.join(" "))

Call site `services/.../src/ops/tag_resource.rs:172-174`:
    if let Some(rejection) = crate::state::tag_set_rejection(TAGS_MEMBER, &input.tags) {
        return Err(TagResourceErrorModeled::ValidationException(rejection).into());
    }
(and `services/.../src/ops/create_rescore_execution_plan.rs:210-218`, `services/.../src/ops/untag_resource.rs:169-171`, `services/.../src/service.rs:111-114` for `refusal_report`).

No bound is justified anywhere: the only length reasoning in reach is vendor's `vendor/smithy-schema-server/src/constraints.rs:63-67` — "ALREADY elided to `MAX_ECHOED_VALUE`: a caller controls its length, and [`Violation::message`] interpolates it as-is, so a value stored unelided would amplify a request into an arbitrarily long response."

Test coverage: `tests/all/tag_character_set.rs:427` (`every_offending_position_is_reported_at_once`) and `tests/all/create_plan_tag_charset.rs:357-359` pin the multi-clause report, but only with 2-3 tags, so no existing test observes a report past 2048 characters.

## Steps to Reproduce

1. Start the emulator and create a rescore execution plan, keeping its ARN (e.g. `CreateRescoreExecutionPlan {Name: "report-plan"}`).
2. Call `TagResource {ResourceARN: <arn>, Tags: [ {Key: "bad!0", Value: "bad!0"}, ... 14 entries, every Key and Value containing '!' (outside the documented tag character set) ]}`.
3. Observe the 400 `ValidationException`: its `Message` is `28 validation errors detected: Value at 'tags.1.member.key' failed to satisfy constraint: Member must consist of Unicode letters, digits, white space and the symbols _ . : / = + - @; ...` — about 4,250 characters, while the model bounds `ValidationException$Message` (`ErrorMessage`) at 2048.
4. Repeat with the 200 tags `TagList`'s `@length 0..200` admits, each Key and Value illegal: the single `Message` grows to roughly 60,000 characters, ~30x the modelled maximum.
5. The same is reachable through `CreateRescoreExecutionPlan {Name: "x", Tags: [...]}` and through `UntagResource {ResourceARN: <arn>, TagKeys: [200 illegal keys]}`.
