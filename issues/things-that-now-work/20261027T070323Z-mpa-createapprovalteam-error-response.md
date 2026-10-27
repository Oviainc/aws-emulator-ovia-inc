Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# The unattachable-policy and repeated-identity refusals quote request values verbatim, so ValidationException$Message exceeds its modelled @length(max: 1000)

_Filed: 2026-10-27T07:03:22Z_
_Operation: CreateApprovalTeam | Class: error-response_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two of CreateApprovalTeam's refusals build their `ValidationException$Message` by interpolating request values whose own modelled bounds are large enough to push the message past `ValidationException$Message`'s `@length(max: 1000)`, and neither goes through the crate's message-bounding helper `crate::state::bounded_message`:

1. `crate::state::unattachable_policy(policy_arn)` embeds the whole `PolicyReference$PolicyArn`. That member targets `QualifiedPolicyArn`, `@length(max: 1224)` with a pattern that admits two 1023-character segments, so a conforming reference of ~1200 characters that the catalog does not hold produces a message of ~1310 characters.
2. The repeated-identity refusal embeds `PrimaryIdentityId` (max 100) and `PrimaryIdentitySourceArn` (max 1000) through `crate::state::repeated_identity`, giving a message of up to ~1200 characters.

In both cases the handler answers a 400 whose `Message` member is outside the bound the model declares for it, i.e. a response shape AWS cannot emit. The crate already states and applies this rule on every other refusal that quotes a long request value back (`cross_account_access_denied`, `invalid_region`), and reasons explicitly about the 1000-character ceiling for the tag family's not-found message; these two paths skip it.

The same repeated-identity message string is built a second time inside `UpdateApprovalTeam` (src/ops/update_approval_team.rs:273-282), where it overflows identically for a 1000-character `PrimaryIdentitySourceArn`; bounding the shared refusal fixes both call sites.

## Evidence

Model bound on the member being written — `smithy_query(query="shape",shape="ValidationException")`: `"Message": {"target": "String", "required": true, "length": {"max": 1000}}`. Model bounds on the values quoted back — `smithy_query(query="shape",shape="QualifiedPolicyArn")`: `"length": {"max": 1224}, "pattern": "^arn:.{1,63}:mpa:::aws:policy/[a-zA-Z0-9_\\.-]{1,1023}/[a-zA-Z0-9_\\.-]{1,1023}/(?:[\\d]+|\\$DEFAULT)$"`; `smithy_query(query="shape",shape="ApprovalTeamRequestApprover")`: `"PrimaryIdentitySourceArn": {... "length": {"max": 1000}}`.

The unbounded refusals — src/state.rs:512-519: `pub fn unattachable_policy(policy_arn: &str) -> ValidationException { ValidationException { message: format!("Policies names a policy version that does not exist: {policy_arn}. Attach a policy version ListPolicies and ListPolicyVersions report.") } }` (no `bounded_message`), reached from src/ops/create_approval_team.rs:331-338 (`if let Some(unattachable) = state.unattachable_policy_reference(...) { return Err(...ValidationException(crate::state::unattachable_policy(unattachable)).into()); }`); and src/ops/create_approval_team.rs:346-359: `return Err(...ValidationException(ValidationException { message: format!("Approvers names the same identity more than once ({repeated}); each approver must be a distinct identity.") })...)` where `repeated` is src/state.rs:2695 `Some(format!("{identity_id} from {source_arn}"))`.

The rule this crate states for exactly this hazard — src/state.rs:2974-2987: "`message` cut to the modelled message bound ([`MAX_EXCEPTION_MESSAGE`]) on a character boundary, so a long value quoted back — a resource ARN (`ApprovalTeamArn` is `length(20..2048)`), a `Region` (`length(max: 1000)`) — cannot push the refusal past the length its own member declares." (`const MAX_EXCEPTION_MESSAGE: usize = 1000;`, src/state.rs:183), applied in src/state.rs:2850-2858 and src/state.rs:3034-3039, and spelled out again in src/state.rs:2899-2903: "`ResourceArn` is `@length(max: 1000)` here — exactly the bound on `ResourceNotFoundException$Message` — so the value always fits the message it becomes and is never cut."

Test coverage: tests/all/authorization_gate.rs:30-32 pins the cut for the cross-account refusal (`const MAX_MESSAGE: usize = 1000;`, "so a refusal naming a resource ARN ... is cut to this bound"), while tests/all/write_path_input_validation.rs and tests/all/approval_team_lifecycle.rs exercise the unattachable-policy and repeated-identity refusals only with short ARNs, so the overflow is unpinned.

## Steps to Reproduce

1. Record the caller's account as the organization's management account (as tests/all/approval_team_lifecycle.rs:192-195 does) so the create reaches its handler.
2. POST /approval-teams with a conforming body whose Policies = [{"PolicyArn": "arn:aws:mpa:::aws:policy/" + "a"*1023 + "/" + "b"*160 + "/1"}] — a value that satisfies QualifiedPolicyArn's pattern and its 1224-character bound but names no catalogued policy.
3. Observe HTTP 400 ValidationException whose Message is ~1310 characters, past the @length(max: 1000) the model declares for ValidationException$Message.
4. Repeat with two Approvers naming the same PrimaryIdentityId and a 1000-character PrimaryIdentitySourceArn: the repeated-identity refusal's Message is ~1200 characters (the identical message in src/ops/update_approval_team.rs:273-282 overflows the same way).
5. Contrast with a cross-account GetApprovalTeam refusal, whose message is cut to 1000 characters by crate::state::bounded_message.
