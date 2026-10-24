Issue-Severity: LOW
Issue-Gap-Class: pagination
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# ListDomains accepts a forged nextToken: any even-length hex string is treated as a cursor instead of being refused

_Filed: 2026-10-24T14:32:39Z_
_Operation: ListDomains | Class: pagination_

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`nextToken` is validated only by hex-decoding it into UTF-8 (`decode_page_token`), so any client-invented even-length hex string is accepted as a cursor and the page silently resumes after the decoded key. Expected: a token the service did not issue is refused with a ValidationException (400) — the handler's own comment claims exactly that ("nextToken is not a token this service issued"). Actual: e.g. `nextToken=646f6d2d7a` (hex of "dom-z") returns HTTP 200 with a page starting after the fabricated key — for key-ordered ids that silently hides domains the caller should have seen, turning a bad request into a wrong-and-successful answer. The same unverified decoder backs ListSubscriptions, and a token issued by one list operation is accepted by the other because the cursor carries no indication of which collection it came from.

## Evidence

Spec: ListDomains/ListSubscriptions are @paginated(inputToken=nextToken, outputToken=nextToken) with an opaque server-issued token; AWS list APIs answer a malformed or unissued pagination token with a validation error rather than a 200 page. Handler: `src/ops/list_domains.rs:190-200` accepts any token `crate::state::decode_page_token` decodes; `src/state.rs:992-1000` implements encode/decode as bare `hex::encode`/`hex::decode` + UTF-8 — there is no issuance marker, signature or collection tag, so the refusal branch only fires for non-hex/non-UTF-8 input. `src/ops/list_subscriptions.rs:196-211` uses the same decoder. Test coverage: `tests/all/domain_subscription_lifecycle.rs:684,800` only feed back tokens the service issued; no test supplies a forged or cross-collection token, so this is unasserted.

## Steps to Reproduce

1. Create three domains via POST /domain so ListDomains has several key-ordered entries.
2. GET /domain?maxResults=1 and note that a nextToken is returned (hex of the last domainId).
3. GET /domain?nextToken=646f6d2d7a (hex of the string "dom-z", a cursor the service never issued).
4. Observe HTTP 200 with a page computed by resuming after the fabricated key (here: an empty `domains` list) instead of a 400 ValidationException rejecting the unissued token.
5. Also feed a nextToken obtained from GET /domain into GET /domains/{domainId}/subscriptions: it is accepted rather than refused.
