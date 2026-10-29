Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 3945994041ceeb2d9433bf9ac91cf7eef00f38b4

# A request member the codec cannot read falls back to the generic refusal body, so the refusal carries no modelled `reason` the crate's own seam exists to guarantee

_Filed: 2026-10-28T19:40:03Z_
_Operation: ListRecommendedActions | Class: error-response_

> _Note: this issue was identified against the code base at commit `3945994041ceeb2d9433bf9ac91cf7eef00f38b4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`src/service.rs` installs a `ModelledValidation` seam in front of `ListRecommendedActions` for one stated reason: the framework's generic refusal body cannot carry `ValidationException`'s modelled members, so every refusal of this operation's input is restated in the operation's own `ValidationException` (with the REQUIRED `reason` and a `fieldList`). The seam only achieves that for refusals `smithy_server_runtime::refusal_of` can describe structurally — a missing required member (`SerdeError::MissingMember`) and a constraint violation (`ProtocolError::Invalid`). Every OTHER codec failure — a member present with the wrong JSON type, e.g. `{"maxResults": "ten"}` or `{"filter": {"actions": 7}}` — arrives as `ProtocolError::Serde(..)` of another kind, for which `refusal_of` returns `None`. `violations()` then answers `None`, the seam hands the request to the generated walk, the walk re-reads the body, fails identically, and renders the framework's generic refusal — the body the crate itself describes as carrying `message` alone.

So the one class of input this crate explicitly calls unreadable ("a body the codec cannot read at all") is the one class whose refusal does NOT state a reason, even though the model declares `ValidationException$reason` REQUIRED and declares the exact value for this class (`ValidationExceptionReason$cannotParse`), and even though this crate already owns the helper for it (`cannot_parse`, used only by the `nextToken` path). The crate's comment admits the fall-through instead of routing it through that helper, so a client that branches on `reason` gets null for a member the model marks required — precisely the failure the seam's own doc says it was written to prevent.

## Evidence

Rule the crate states (src/service.rs:86-98):
  "/// WHY this is a seam and not a line in the handler: `ValidationException` models\n/// `reason` (REQUIRED) and `fieldList` beside `message`, and the framework\n/// validates every modelled constraint ... BEFORE\n/// a handler runs, rendering a violation as the protocol's generic refusal body,\n/// which carries `message` alone. A client deserializing that error gets null for a\n/// member the model marks required, so `reason`-based branching and per-field\n/// reporting are impossible. The refusal is therefore stated HERE"

The path that breaks it / admits the gap (src/service.rs:107-110):
  "/// `None` means the request carries no structured refusal —\n/// either it is valid, or it failed in a way that has no member-level form (a\n/// body the codec cannot read at all), which the generated walk renders itself."
and src/service.rs:128-139:
  "// Absence is found while READING the request, so it arrives as a\n// codec error; `refusal_of` gives it the same structured form a\n// constraint violation has.\nErr(e) => aws_smithy_schema_server::ProtocolError::Serde(e),\n... match smithy_server_runtime::refusal_of(&error) {\n    Some(Refusal::Invalid(violations)) if !violations.is_empty() => Some(violations),\n    _ => None,\n}"

Proof that only two error kinds get a structured form (vendor/smithy-server-runtime/src/pre_dispatch.rs:179-193):
  "pub fn refusal_of(error: &ProtocolError) -> Option<Refusal> {\n    match error {\n        ProtocolError::Serde(::aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => Some(...),\n        ProtocolError::Invalid(violations) => Some(Refusal::Invalid(violations.clone())),\n        ProtocolError::Unauthenticated { .. } => Some(Refusal::Unauthenticated),\n        _ => None,\n    }"
pinned by vendor/smithy-server-runtime/src/pre_dispatch.rs:518-521:
  "assert_eq!(refusal_of(&ProtocolError::Malformed(\"x\".into())), None);\nassert_eq!(refusal_of(&ProtocolError::Serde(SerdeError::custom(\"other\"))), None);"

The helper this path skips, which the crate already has for exactly this class (src/ops/list_recommended_actions.rs:176-178 and 263-270):
  "/// `ValidationExceptionReason$cannotParse` — the request carries a value this\n/// service could not read at all (here: a malformed `nextToken`).\nconst REASON_CANNOT_PARSE: &str = \"cannotParse\";"
  "/// A 400 `ValidationException` for a value the service could not read at all —\n/// `ValidationExceptionReason$cannotParse` — naming the member it could not read.\nfn cannot_parse(...)"

Model side: `ValidationException` members are `message` (required), `reason` (required, target `ValidationExceptionReason`, doc "Provides a single, overarching explanation for the validation failure"), `fieldList`; `ValidationExceptionReason` values are `["unknownOperation", "cannotParse", "fieldValidationFailed", "other"]` (smithy_query shape=ValidationException,ValidationExceptionReason).

Test coverage: the crate's refusal suites only drive member-level refusals — `every_refused_member_is_stated_with_a_reason_and_a_field_list` (tests/all/list_recommended_actions.rs:745), `the_reason_states_whether_the_refused_value_could_be_read` (tests/all/list_recommended_actions.rs:2371, which builds its "unreadable" cases from damaged `nextToken` strings only, lines 2445-2451) and `max_results_outside_the_published_range_is_refused` (line 605). No case sends a member with the wrong JSON type, so nothing pins the current fall-through.

## Steps to Reproduce

1. POST / with `X-Amz-Target: AWSBillingAndCostManagementRecommendedActions.ListRecommendedActions` and body `{"maxResults": "ten"}` (a member present with the wrong JSON type).
2. Follow src/service.rs:116-139: `ListRecommendedActionsInput::deserialize` fails with a non-`MissingMember` `SerdeError`, wrapped as `ProtocolError::Serde(e)`.
3. Follow vendor/smithy-server-runtime/src/pre_dispatch.rs:179-193: `refusal_of` matches none of the three arms and returns `None`, so `ModelledValidation::violations` returns `None` (src/service.rs:136-139).
4. src/service.rs:186-189 therefore runs the generated walk, which re-reads the same body, fails the same way, and renders the framework's generic refusal — the body src/service.rs:90-92 describes as carrying "`message` alone".
5. Observe the response has no `reason` member, although the model marks `ValidationException$reason` required and declares `cannotParse` for a value that could not be read, and although this crate already has `cannot_parse` (src/ops/list_recommended_actions.rs:265) for that class.
