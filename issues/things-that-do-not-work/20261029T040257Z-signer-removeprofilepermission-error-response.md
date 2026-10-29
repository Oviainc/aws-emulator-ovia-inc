Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9928f8d1a376eb4ec72e185f331df5d236ffae0d

# A RemoveProfilePermission request that omits the required revisionId query parameter answers 501 NotImplemented instead of the modelled 400

_Filed: 2026-10-29T04:02:57Z_
_Operation: RemoveProfilePermission | Class: error-response_

> _Note: this issue was identified against the code base at commit `9928f8d1a376eb4ec72e185f331df5d236ffae0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`RemoveProfilePermission` binds `revisionId` as a `@required` `@httpQuery` member. Because the REST router refuses to match a route whose `@required` query bindings are not all on the request, `DELETE /signing-profiles/{profileName}/permissions/{statementId}` without `?revisionId=…` resolves to no operation and is answered with the restJson1 routing default, 501 NotImplemented — a status that says this operation was never written, for a request that merely left a required parameter out. The handler and its modelled `ValidationException` (400) are never reached.

The crate states exactly this defect and its remedy in `service.rs` and applies the remedy to `GET /revocations` only; the permissions route needing the same treatment does not get it, even though the handler below it is fully implemented and reads `input.revision_id` as its optimistic-concurrency precondition.

## Evidence

Model (smithy_query operation=RemoveProfilePermission): `"revisionId": {"target": "String", "httpQuery": "revisionId", "required": true}`, `http: {"method": "DELETE", "uri": "/signing-profiles/{profileName}/permissions/{statementId}"}`, errors include `ValidationException (httpError 400)`.

Router (vendor/smithy-schema-server/src/rest_xml.rs:561-575, shared by restJson1 through rest_json.rs:159): `RequiredBinding::Query(name) => request_query.iter().any(|&(k, _)| k == *name) ... if !present { return None; }` — "the route must not match at all, because the operation could not be invoked without it".

The crate's own statement of the rule and the fix, services/service-com-amazonaws-signer-wallabyservice/src/service.rs:337-353: "A request leaving one out resolves to NO operation, and the protocol default for that is 501 `NotImplemented` — which says nobody implemented the operation, when what actually happened is that the request left a required parameter out. // Naming the operation here hands such a request to the operation it addressed, so the centralized walk's own boundary validator refuses it with the modelled 400 naming each absent member." followed by `if addresses_revocation_status(req) { return self.entry.routing_table().get(GET_REVOCATION_STATUS); }` — the only route so named.

The handler that never runs: src/ops/remove_profile_permission.rs:202-218 — "`revisionId` is required here: a removal is applied only while the revision it was issued against is still current" / `if policy.revision_id != input.revision_id { ... ConflictException ... }`.

Test coverage: tests/all/profile_permissions.rs always sends the parameter (the suite's remove helper builds `…/permissions/{statementId}?revisionId=…`); no test pins the omitted-revisionId answer.

## Steps to Reproduce

1. PUT /signing-profiles/Perm_Route with {"platformId":"AWSLambda-SHA384-ECDSA"} to create a profile.
2. POST /signing-profiles/Perm_Route/permissions with {"statementId":"s1","action":"signer:StartSigningJob","principal":"123456789012"} and note the revisionId.
3. Send DELETE /signing-profiles/Perm_Route/permissions/s1 with no query string.
4. Observe HTTP 501 NotImplemented instead of the modelled 400 ValidationException naming the absent revisionId; the same request with ?revisionId=<revision> is served normally, proving the 501 is a routing artefact, not the operation being unimplemented.
