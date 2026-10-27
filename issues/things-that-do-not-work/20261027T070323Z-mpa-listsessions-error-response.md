Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# A SessionStatus filter refusal quotes the whole Filter$Value, so ValidationException$Message exceeds its modelled @length(max: 1000)

_Filed: 2026-10-27T07:03:22Z_
_Operation: ListSessions | Class: error-response_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListSessions refuses a `SessionStatus` filter whose `Value` is not a token the enum declares by building a message that embeds that value VERBATIM and never passes it through the crate's own message-bounding helper (`crate::state::bounded_message`). `Filter$Value` is modelled `@length(max: 1000)`, and the refusal adds ~140 characters of prose plus the joined token catalog around it, so a request carrying a 1000-character filter value is answered with a `ValidationException` whose `Message` is roughly 1140 characters — past the `@length(max: 1000)` the model declares for `ValidationException$Message`. The response therefore carries a member outside its own modelled bound, which AWS can never emit.

The refusal is reachable with nothing stored at all: the handler compiles the filters BEFORE it resolves the team ("The filters are read before any session is ... then the request's own shape (its filters and its token); then, below, whether the team exists"), so any ARN in the caller's account plus one oversized filter value reaches the message builder.

The crate already owns this rule and applies it on the paths that quote a long request value back (`cross_account_access_denied`, `invalid_region`); `ListSessions`' own `validation()` constructor bypasses it.

## Evidence

Model bound on the member being written — `smithy_query(query="shape",shape="ValidationException")`: `"Message": {"target": "String", "required": true, "length": {"max": 1000}}`. Model bound on the value being quoted — `smithy_query(query="shape",shape="Filter")`: `"Value": {"target": "String", "length": {"max": 1000}, "doc": "<p>Value to use for filtering..."}`.

The unbounded message — src/ops/list_sessions.rs:465-474: `fn declared_status(index: usize, value: &str) -> Result<&str, ListSessionsError> { if SESSION_STATUS_VALUES.contains(&value) { return Ok(value); } Err(validation(format!("Filters[{index}].Value '{value}' is not a status a session can be in; the {SESSION_STATUS_FIELD} filter accepts {}.", SESSION_STATUS_VALUES.join(" | ")))) }`, raised through src/ops/list_sessions.rs:620-625: `fn validation(message: String) -> ListSessionsError { ListSessionsError::Modeled(ListSessionsErrorModeled::ValidationException(ValidationException { message })) }` — no call to `bounded_message`.

The rule this crate states for exactly this hazard — src/state.rs:2974-2987: "`message` cut to the modelled message bound ([`MAX_EXCEPTION_MESSAGE`]) on a character boundary, so a long value quoted back — a resource ARN (`ApprovalTeamArn` is `length(20..2048)`), a `Region` (`length(max: 1000)`) — cannot push the refusal past the length its own member declares." and src/state.rs:183: `const MAX_EXCEPTION_MESSAGE: usize = 1000;`. It is applied by src/state.rs:2850-2858 (`cross_account_access_denied` → `bounded_message(format!(...))`) and src/state.rs:3034-3039 (`invalid_region`), and the same reasoning is spelled out for the tag family in src/state.rs:2899-2903: "`ResourceArn` is `@length(max: 1000)` here — exactly the bound on `ResourceNotFoundException$Message` — so the value always fits the message it becomes and is never cut."

Reachability before any lookup — src/ops/list_sessions.rs:216-222: "The filters are read before any session is: a request carrying one this operation cannot apply is refused ... then the request's own shape (its filters and its token); then, below, whether the team exists." followed by `let predicates = compile_filters(input.filters.as_deref().unwrap_or_default())?;`.

Test coverage: tests/all/session_listing.rs drives the filter refusals but no row carries a maximal `Filter$Value`, so the overflow is unpinned; tests/all/authorization_gate.rs:30-32 pins the cut for the other direction ("`AccessDeniedException$Message` is `@length(max: 1000)`, so a refusal naming a resource ARN (`ApprovalTeamArn` is `length(20..2048)`) is cut to this bound" / `const MAX_MESSAGE: usize = 1000;`).

## Steps to Reproduce

1. Point an MPA client at the emulator.
2. POST /approval-teams/arn%3Aaws%3Ampa%3Aus-east-1%3A<caller-account>%3Aapproval-team%2Fany-team/sessions/?List with body {"MaxResults": 20, "Filters": [{"FieldName": "SessionStatus", "Operator": "EQ", "Value": "<1000 'x' characters>"}]} (the team need not exist: the filters are compiled first).
3. Observe HTTP 400 ValidationException whose Message is ~1140 characters long (it contains the 1000-character value verbatim).
4. Compare with the bound the model declares for ValidationException$Message (@length(max: 1000)) and with src/state::bounded_message, which every other refusal that quotes a long request value goes through.
