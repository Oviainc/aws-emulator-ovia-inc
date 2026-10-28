Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe

# The synthetic ValidationException spells its message member `message`, which this crate's own rule says is "no message at all" to an awsJson1_1 client

_Filed: 2026-10-28T18:32:12Z_
_Operation: ListAdminAccountsForOrganization | Class: error-response_

> _Note: this issue was identified against the code base at commit `86a1fe66fd012b6ef331ae9dc1d2d842007fb4fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every refusal this crate raises through `crate::scaffolded::Scaffolded::validation` is serialized from `VALIDATION_EXCEPTION_SCHEMA`, whose single member is NAMED `message` (lowercase) and carries no `with_json_name`. On awsJson1_1 — this service's protocol — the member NAME is the JSON key, so those bodies answer `{"message": "..."}`.

The crate states the opposite rule for itself, right next to that schema: the sibling synthetic shape `AccessDeniedException` is deliberately spelled `Message` (and additionally pins `with_json_name("Message")`) precisely because "every error shape in this service's model declares its one member as `Message`" and "a lowercase name would answer a body with no message member at all as far as a client is concerned". The model agrees: `com.amazonaws.fms#InvalidInputException` and `com.amazonaws.fms#ResourceNotFoundException` each declare exactly one member, `Message`.

So the one error shape this service invents for a bad request is the only client error of the service whose message a client/SDK cannot read. It is not a rare path: nine handlers return it — the bad-`NextToken` refusal of ListAdminAccountsForOrganization, ListPolicies, ListMemberAccounts, ListAppsLists, ListProtocolsLists and ListComplianceStatus, and the malformed-`ResourceArn` refusal of TagResource, UntagResource and ListTagsForResource. In each case the handler composes a message naming what was wrong (`list_tags_for_resource.rs:181` even passes the reason built from the offending ARN), and that text reaches the wire under a key no SDK reads.

This is the half of the already-fixed "FMS error responses carry missing or non-conforming Message values" issue that was left behind: that fix moved the authorization refusal to `Message` but not the request-shape refusal, and the crate's test helpers still carry the now-stale claim that the authorization refusal spells it lowercase.

Filed against ListAdminAccountsForOrganization as the first checklist operation whose handler returns it; the defect is in the shared `crate::scaffolded` shape and shows on all nine.

## Evidence

The rule, stated by the crate for itself — services/service-com-amazonaws-fms-awsfms-20180101/src/scaffolded.rs:192-201:
```
/// The `Message` member of the synthetic `AccessDeniedException` error shape.
///
/// Spelled `Message`, not `message`: this shape is not a framework signal but
/// the stand-in for an error the LIVE service answers with, and every error
/// shape in this service's model declares its one member as `Message` (an
/// `ErrorMessage`), which on the awsJson / rpcv2Cbor wire is the key clients and
/// SDKs read the exception message from. The member name IS that key on these
/// protocols (`@jsonName` is not consulted by the awsJson family), so a
/// lowercase name would answer a body with no message member at all as far as a
/// client is concerned.
static ACCESS_DENIED_EXCEPTION_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#AccessDeniedException$Message",
        "smithy.framework",
        "Message",
    ),
    ShapeType::String,
    "Message",
    0,
)
.with_xml_name("Message")
.with_json_name("Message");
```

The shape that breaks it — src/scaffolded.rs:129-140 (member name `message`, no json name):
```
/// The `message` member of the synthetic `ValidationException` error shape.
static VALIDATION_EXCEPTION_MESSAGE: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#ValidationException$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");
```

The crate's own tests confirm the body really carries the lowercase key — tests/all/administrator_gated_operations.rs:56-58:
```
/// The `message` member of an awsJson1_1 error envelope, under either casing: the
/// modelled error shapes of this service spell it `Message`, the synthetic
/// authorization and request-shape refusals `message`.
```
(and the same stale sentence in tests/all/compliance_status.rs:108-110, which the AccessDenied fix has since contradicted), versus the intolerant reader and the rule in tests/all/error_message_members.rs:9-13:
```
//! 1. Every client error this service answers carries its message under the
//!    member name the model declares for it — `Message` (every error shape in
//!    `com.amazonaws.fms` declares exactly one member, `Message`, targeting
//!    `ErrorMessage`). On awsJson1_1 the MEMBER NAME is the JSON key, so a body
//!    that spells it any other way has, to a client, no message at all.
```

The model side: `smithy_query(query="shape", shape="InvalidInputException")` → `"members": {"Message": {"target": "ErrorMessage"}}` (same for `ResourceNotFoundException`).

The nine handlers that raise it:
- src/ops/list_admin_accounts_for_organization.rs:190 `return Err(crate::scaffolded::Scaffolded::validation("NextToken is not a pagination token issued by ListAdminAccountsForOrganization"));`
- src/ops/list_policies.rs:197, src/ops/list_member_accounts.rs:181, src/ops/list_apps_lists.rs:201, src/ops/list_protocols_lists.rs:197, src/ops/list_compliance_status.rs:204 — same bad-token refusal
- src/ops/list_tags_for_resource.rs:181-183, src/ops/tag_resource.rs:182-183, src/ops/untag_resource.rs:171-172 — `if let Some(reason) = crate::state::arn_format_error(&input.resource_arn) { return Err(crate::scaffolded::Scaffolded::validation(reason)); }`

Test coverage today: every suite that asserts this refusal checks only the error CODE (tests/all/pagination_token_decoding.rs:89,96 `refusal: "ValidationException"`; tests/all/tagging_administrator_gate.rs:226) and reads the text through the casing-tolerant `error_message` helper, so nothing pins the lowercase spelling and a fix to `Message` leaves every existing assertion passing.

## Steps to Reproduce

1. Boot the emulator and associate a Firewall Manager administrator (`AssociateAdminAccount`).
2. Call `ListAdminAccountsForOrganization` with `{"NextToken": "zzzz"}` (a value that is not a token this service minted).
3. Observe HTTP 400 with `__type` `ValidationException` and a body whose only message member is the lowercase key `message` — e.g. `{"message": "NextToken is not a pagination token issued by ListAdminAccountsForOrganization"}`.
4. Call `TagResource` with `{"ResourceArn": "this-is-not-an-arn-at-all", "TagList": []}` and observe the same lowercase `message` key.
5. For contrast, call any gated operation with no administrator associated (e.g. `ListPolicies`) and observe the AccessDeniedException body carrying `Message`, and `GetPolicy` with an unknown id carrying `Message` — the two spellings a single client must now handle.
