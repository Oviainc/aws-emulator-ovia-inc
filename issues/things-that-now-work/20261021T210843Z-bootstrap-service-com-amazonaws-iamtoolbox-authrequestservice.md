Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the com-amazonaws-iamtoolbox-authrequestservice emulator

_Filed: 2026-10-21T21:08:43Z_
_Operation: service-com-amazonaws-iamtoolbox-authrequestservice | Class: stub_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service is a fresh scaffold: its single bound operation, `GetRequestAuthorizationDetails` (`src/ops/get_request_authorization_details.rs`), still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns HTTP 501 `NotImplemented`, and `src/state.rs` holds an empty `ComAmazonawsIamtoolboxAuthrequestserviceState { /* TODO */ }`. Because the service models exactly ONE operation (a `@readonly`, `@paginated` read), the first slice is that read plus the state it reads from.

## Happy path to implement (in order)

1. **Seed the authorization-details store** (no modeled write op exists — the record store IS the first thing to build). A record is keyed by `authorizationId` (lowercase alphanumeric, per the `^[0123456789abcdefghijklmnopqrstuvwxyz]+$` pattern on the input label) and holds the request context, the ordered evaluation list, the evaluated policy list, and a creation instant (real AWS keeps details for at least 24h). Records must be insertable into the shared engine state (fixture/seed entries populated when the service state is created, and/or a test helper), so that the read below answers from persisted state rather than from a value built inside the handler.
2. **`GetRequestAuthorizationDetails` — found case.** Input: `authorizationId` (@httpLabel, required), `nextToken` (@httpQuery). On a hit, return HTTP 200 with `requestContext` (map<string,document>, required), `evaluations` (list<Evaluation>, required), `policies` (list<PolicyInfo>, required), and `nextToken` only when a further page exists. Every field must be read out of the stored record for that id — two different seeded ids must return two different bodies, and the same id must return the same body on repeated calls (the operation is `@readonly`: it must not mutate or consume state).
3. **`GetRequestAuthorizationDetails` — pagination.** `@paginated(inputToken=nextToken, outputToken=nextToken, items=evaluations)`: slice the stored `evaluations` list into pages, emit an opaque `nextToken` that resolves back to the offset in that record's list, and omit `nextToken` on the last page. `requestContext` and `policies` are request-level and repeat on every page. Walking pages to exhaustion must yield exactly the stored evaluations, in order, with no duplicates or gaps.
4. **`GetRequestAuthorizationDetails` — unknown / expired id.** An id with no record (or one older than the retention window) must be `ResourceNotFoundException` HTTP 404 with a `message`, not an empty 200 and not a 501.
5. **`GetRequestAuthorizationDetails` — malformed input.** A `nextToken` that does not decode, or does not belong to the requested `authorizationId`, must be `ValidationException` HTTP 400 with a `message`. (The `authorizationId` pattern is a modeled constraint and is refused by the framework before `handle` runs.)

Every one of these responses must be computed from the persisted record store: no canned bodies, no echoing the input id back inside a fabricated evaluation, and no handler-local construction of `Evaluation`/`PolicyInfo` values independent of state. Delete the `SCAFFOLDED, NOT IMPLEMENTED` marker line only once the handler reads real state.

## State Model

- `ComAmazonawsIamtoolboxAuthrequestserviceState { authorization_details: HashMap<String, AuthorizationDetailsRecord> }` — keyed by `authorizationId`.
- `AuthorizationDetailsRecord { authorization_id: String, request_context: HashMap<String, Document>, evaluations: Vec<Evaluation>, policies: Vec<PolicyInfo>, created_at: SystemTime }` — `created_at` drives the ≥ 24h retention / 404-on-expiry behaviour.
- `Evaluation { action: String (required), resource: String (required), context: HashMap<String, Document>, evaluated_effect: EvaluatedEffect (ALLOW | EXPLICIT_DENY | IMPLICIT_DENY), matched_policies: Vec<MatchedPolicy> }`.
- `MatchedPolicy { uri: String (required), matched_statements: Vec<MatchedStatement> }`; `MatchedStatement { sid: Option<String>, evaluated_effect: StatementEffect (ALLOW | DENY) }`.
- `PolicyInfo { policy_type: PolicyType (IDENTITY_BASED_POLICY | RESOURCE_BASED_POLICY | PERMISSIONS_BOUNDARY | SESSION_POLICY | SERVICE_CONTROL_POLICY | RESOURCE_CONTROL_POLICY | VPC_ENDPOINT_POLICY), inline: Option<bool>, uri: Option<String>, attached_to: Vec<AttachedTo> }`; `AttachedTo { arn: Option<String> }` (arn pattern `^arn:[^:]*:[^:]*:[^:]*:[^:]*:.*$`).
- Pagination cursor: an opaque token encoding `(authorization_id, offset_into_evaluations)`; resolved against the stored record, rejected with 400 if it does not parse or targets a different id.

There is nothing else in this service's model, so this slice IS the service's end-to-end workflow; refinements (exact token encoding, page-size policy, retention sweeping, `AccessDeniedException` conditions) are follow-up gap issues.

## Evidence

`services/service-com-amazonaws-iamtoolbox-authrequestservice/OPERATIONS.md` lists exactly 1 operation: `GetRequestAuthorizationDetails`, `GET /authorization-details/{authorizationId}`, restJson1, sigv4 name `iam`, traits `@paginated(inputToken=nextToken, outputToken=nextToken, items=evaluations)` and `@readonly`; input `authorizationId` (required, @httpLabel) + `nextToken` (@httpQuery); output `requestContext`/`evaluations`/`policies` (all required) + `nextToken`; errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ValidationException 400. The Smithy closure for that operation supplies the shape details cited in the State Model (Evaluation, MatchedPolicy, MatchedStatement, PolicyInfo, AttachedTo, the EvaluatedEffect/StatementEffect/PolicyType enums, the `authorizationId` and `arn` patterns). The handler `src/ops/get_request_authorization_details.rs` lines 4 and 179 show the `SCAFFOLDED, NOT IMPLEMENTED` header and `Err(crate::scaffolded::Scaffolded::not_implemented("GetRequestAuthorizationDetails"))`; `src/state.rs` line 8 shows the state struct is still an empty TODO. The operation's own modeled docs (reproduced in the handler header) ground the retention rule ("Authorization details are available for at least 24 hours after the denial") and the 404 semantics ("do not exist in this region or have expired"). No tests assert any of this — only the generated `tests/state_roundtrip.rs` example exists.

## Steps to Reproduce

1. Start the emulator with this service mounted and seed one authorization-details record, id `abc123def`, with a request context (e.g. aws:PrincipalArn), several evaluations (mixed ALLOW / EXPLICIT_DENY / IMPLICIT_DENY) and a couple of evaluated policies.
2. GET /authorization-details/abc123def — expect HTTP 200 whose `requestContext`, `evaluations` and `policies` match exactly what was seeded (today: HTTP 501 NotImplemented).
3. Repeat the same GET — expect a byte-identical body: the read must not consume or mutate state.
4. Seed a second record `zzz999` with different evaluations and GET it — expect its own body, proving the response is looked up by id and not canned.
5. Walk pagination: GET /authorization-details/abc123def, then re-GET with `?nextToken=<returned token>` until `nextToken` is absent — expect the concatenated `evaluations` to equal the seeded list in order, with no duplicates and no gaps.
6. GET /authorization-details/nosuchid — expect HTTP 404 `ResourceNotFoundException` with a `message`.
7. GET /authorization-details/abc123def?nextToken=garbage — expect HTTP 400 `ValidationException` with a `message`.
