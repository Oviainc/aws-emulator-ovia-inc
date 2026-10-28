Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# ListQuotes refuses a foreign page token with the framework's synthetic ValidationException, so its body carries `message` instead of the modelled `Message`

_Filed: 2026-10-28T16:06:28Z_
_Operation: ListQuotes | Class: error-response_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every paged list in this crate refuses a `NextToken` it did not issue for that list by returning the MODELLED `com.amazonaws.outposts#ValidationException`, whose only member is `Message`. `ListQuotes` is the single exception: because its own operation does not list `ValidationException`, the handler reaches for the scaffold wrapper's synthetic `smithy.framework#ValidationException` instead of widening the operation's error enum to the shape the SERVICE models. The synthetic shape's member is named `message` (lowercase) on restJson1, so the refusal body differs from every other list's: a client (or fixture) reading `$.Message` gets nothing back from ListQuotes while the same refusal from ListSites/ListOutposts/ListOrders/ListCapacityTasks/ListAssets fills it. The status (400) and wire code (`ValidationException`) happen to coincide, which is why the existing test does not catch it. `crate::scaffolded` itself documents that this is the wrong channel and names the mechanism to use instead (`extra_errors`, "only for widening in a shape the SERVICE models that this OPERATION does not list" — exactly ListQuotes' situation).

## Evidence

The outlier — services/service-com-amazonaws-outposts-outpostsolafservice/src/ops/list_quotes.rs:150-166:
```
// A token this service did not issue for THIS list is a bad request, not an
// empty page — the rule every other list in this service applies to theirs. This
// operation's model declares no client-input error, so the refusal travels on the
// service's own 400 `ValidationException` channel rather than as an invented code.
...
        crate::scaffolded::Scaffolded::validation(crate::state::page_token_report(
            Self::NAME,
        ))
```
(`ListQuotesErrorModeled` carries only `AccessDeniedException` / `InternalServerException` — list_quotes.rs:82-85 — and list_quotes.rs:159 is the ONLY use of `Scaffolded::validation` in the whole crate.)

The sibling applying the same rule — src/ops/list_sites.rs:157-168:
```
// The resume point the caller's token names. A token this service did not
// issue for THIS list is a bad request, not an empty page.
...
    crate::state::decode_page_token(crate::state::SITE_PAGE, token).ok_or_else(
        || {
            ListSitesErrorModeled::ValidationException(ValidationException {
                message: Some(crate::state::page_token_report("ListSites")),
            })
```

The two shapes are not the same on the wire. The modelled one (smithy_query shape=ValidationException): `"members": {"Message": {"target": "ErrorMessage", ...}}`. The synthetic one — src/scaffolded.rs:53-63 with its own comment:
```
/// `with_xml_name("Message")` matches the casing the restXml / awsQuery error
/// envelopes use, so the same value renders correctly on the XML protocols and as
/// lowercase `message` on the JSON/CBOR ones.
static VALIDATION_EXCEPTION_MESSAGE: Schema = Schema::new_member(... "message", 0)
```
(this service is restJson1 — OPERATIONS.md:10 `**Protocol:** restJson1`).

The crate's own instruction, src/scaffolded.rs:215-220:
```
/// If this service's model declares its own `ValidationException` (or another
/// client-input error) use THAT: the modeled shape may carry members beyond
/// `message` ... `#[operation_error]`'s `extra_errors` is only for widening in a shape the
/// SERVICE models that this OPERATION does not list.
```

The crate's tests know the two keys are different — tests/all/error_message_rules.rs:693-696: "The error message a body carries under `key` (`Message` for a modelled error shape, `message` for the framework's synthetic envelope)" — and the ListQuotes token test pins only status + code, so the divergence is unpinned: tests/all/quote_listing.rs:291-339 builds `refusal(..., 400, "ValidationException")` rows and `expect_refusals` asserts `(reply.status, reply.error_code.as_str())` only (tests/all/site_and_outpost_lifecycle.rs:442-460).

## Steps to Reproduce

1. Create a quote: POST /quotes with {"CountryCode":"US","RequestedCapacities":[]}.
2. Replay a token stamped for another list against ListQuotes: GET /quotes?NextToken=1%23%23sites (a value the modelled Token pattern ^(\d+)##(\S+)$ accepts, so it reaches the handler).
3. Observe 400 with code ValidationException and a body whose message lives under the key `message`.
4. Do the same against a sibling list: GET /sites?NextToken=1%23%23quotes — the body carries the same text under `Message`.
5. Read $.Message on both replies: it is populated for ListSites and empty for ListQuotes.
