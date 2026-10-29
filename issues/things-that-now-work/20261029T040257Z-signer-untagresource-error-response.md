Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 9928f8d1a376eb4ec72e185f331df5d236ffae0d

# An UntagResource request that supplies no tagKeys answers 501 NotImplemented, so the modelled 400 wording the crate writes for that member is unreachable

_Filed: 2026-10-29T04:02:57Z_
_Operation: UntagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `9928f8d1a376eb4ec72e185f331df5d236ffae0d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` binds `tagKeys` as a `@required` `@httpQuery` member. The REST router only matches a route when every `@required` query binding is present on the request, so a `DELETE /tags/{resourceArn}` carrying no `tagKeys` parameter — which on the wire is the SAME request as one carrying an empty list, the point this crate makes in `missing_list_member_constraint` — resolves to NO operation and is answered with the routing default for restJson1, 501 NotImplemented. It never reaches the boundary validator, so `UntagResource`'s modelled `BadRequestException` (400) is never answered and the sentence this service writes for exactly this case (`tagKeys must not be null or empty`) can never be produced over HTTP.

This crate already owns the fix and applies it to the only other operation with the same shape: `override_operation` names `GetRevocationStatus` for `GET /revocations` precisely so that a request leaving a required query parameter out is handed to the operation it addressed instead of answering 501. `DELETE /tags/{resourceArn}` is not given the same treatment, even though `service.rs` declares `UntagResource`'s `tagKeys` as one of the two members the rewording rule covers, and its own unit test asserts the reworded refusal for `tagKeys`.

## Evidence

Model (smithy_query operation=UntagResource): `"tagKeys": {"target": "TagKeyList", "httpQuery": "tagKeys", "required": true, "length": {"min": 1, "max": 200}, "type": "list"}`, errors `BadRequestException (httpError 400)`.

Router (vendor/smithy-schema-server/src/rest_xml.rs:554-575, used by restJson1 via rest_json.rs:159): "Returns `Some(n)` when **every** required binding is present ... or `None` when any is missing — in which case the route must not match at all" / `if !present { return None; }`.

The crate states the consequence itself — services/service-com-amazonaws-signer-wallabyservice/src/service.rs:337-350: "so the REST router's required-binding check matches that route only for a request that carries every one of those query parameters. A request leaving one out resolves to NO operation, and the protocol default for that is 501 `NotImplemented` — which says nobody implemented the operation, when what actually happened is that the request left a required parameter out. // Naming the operation here hands such a request to the operation it addressed, so the centralized walk's own boundary validator refuses it with the modelled 400" — and then wires that fix for ONE route only, service.rs:351-353: `if addresses_revocation_status(req) { return self.entry.routing_table().get(GET_REVOCATION_STATUS); }`.

Both sides of the inconsistency inside this crate: service.rs:197-210 declares the rule covers both members — "the only two this model has, `GetRevocationStatus`'s `certificateHashes` and `UntagResource`'s `tagKeys`" with `QUERY_BOUND_LIST_INPUTS = &[GET_REVOCATION_STATUS_INPUT_SCHEMA, UNTAG_RESOURCE_INPUT_SCHEMA]`; and the unit test service.rs:463-468 asserts the rewording for both: `for member in ["certificateHashes", "tagKeys"] { let refused = missing_list_member_refusal(&[violation(member, ViolationKind::MissingRequiredMember, None)]).unwrap_or_else(...)`.

Test coverage: only the GetRevocationStatus half is exercised end to end — tests/all/supplied_nothing_refusal_wording.rs:18-19 "the members are those the model binds, read off the one operation of this service that binds a list to the query string", and its cases name `certificateHashes` only (lines 254-256). tests/all/resource_tags.rs:226-236 always builds `DELETE /tags/{arn}?tagKeys=…` with at least one key, so no test pins the no-tagKeys answer.

## Steps to Reproduce

1. PUT /signing-profiles/Untag_Nothing with {"platformId":"AWSLambda-SHA384-ECDSA"} to create a profile and note its arn.
2. Send DELETE /tags/<url-encoded profile arn> with NO query string at all (the wire form of an empty tagKeys list).
3. Observe HTTP 501 NotImplemented from the routing default instead of the modelled 400 BadRequestException carrying "tagKeys must not be null or empty" (the sentence crate::service::missing_list_member_constraint builds and the service.rs unit test asserts for tagKeys).
4. Compare with the sibling route GET /revocations missing one required query parameter, which answers the modelled 400 because override_operation names GetRevocationStatus for it.
