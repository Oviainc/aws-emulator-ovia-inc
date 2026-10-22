Issue-Severity: MEDIUM
Issue-Gap-Class: behavior
Issue-Commit: pending

# The REST route table answers 501 for a request that omits a `@required` query parameter

_Filed: 2026-10-22T01:00:00Z_
_Operation: core/router (vendor/smithy-schema-server REST route lookup) | Class: behavior_

> _Filed at the request of the merge gate's code reviewer while implementing
> `GetStaticMap` in `service-com-amazonaws-geomaps-mapsservice`, which had to work
> around this in its own `service.rs`._

## Description

`route_lookup` in `vendor/smithy-schema-server/src/rest_xml.rs` (shared by the
`restJson1` and `restXml` protocols) **rejects a candidate route when any of the
operation's `@required` header/query bindings is absent from the request**:

```rust
// Required-binding disambiguation: every `@required` header/query
// binding the operation declares must actually be on the request, or
// this operation is not a legal target for it.
let required_score =
    match required_binding_score(route.required, &parse_query_pairs(query), has_header) {
        Some(n) => n,
        None => continue,
    };
```

That rule exists to disambiguate S3's overloaded routes (`PutObject` vs
`CopyObject` vs `UploadPart` on `PUT /{Bucket}/{Key+}`), and for that purpose it
is right. But when only ONE operation is bound to the path, a missing required
query parameter makes the request resolve to **no operation at all**, and the
router answers the protocol's unknown-operation response — HTTP 501
`NotImplemented` for a REST service — instead of the 400 `ValidationException`
the service's own boundary validation would produce.

AWS answers a missing required parameter with a 400. A 501 is also the emulator's
"nobody has written this operation yet" signal, so the wrong answer here reads as
an unimplemented operation rather than a bad request.

## Steps to Reproduce

1. Start the emulator.
2. `GET /v2/static/map?center=0,0&zoom=5` against the `geo-maps` endpoint
   (`GetStaticMap` declares `Width` and `Height` as `@required`
   `@httpQuery` members, and this request omits both).
3. Expected: HTTP 400 `ValidationException` naming the missing members.
   Actual (with no per-service workaround): HTTP 501 `NotImplemented`,
   `{"__type":"NotImplemented","message":"operation GET /v2/static/map is not
   implemented in this emulator"}`.

## Evidence

`services/service-com-amazonaws-geomaps-mapsservice/src/service.rs` works around
it per service, in `override_operation`:

```rust
// The model's own routing table resolves everything it can.
if self.entry.protocol().routing_key(req).is_ok() {
    return None;
}
// What it cannot: the REST route lookup rejects a candidate route whose
// `@required` query bindings are absent from the request, so
// `GET /v2/static/{FileName}` without `width`/`height` resolves to NO
// operation and would be answered as an unknown route. ...
```

Every REST service with a `@required` `@httpQuery` or `@httpHeader` member needs
the same claim, so the workaround will be copied into service after service
until the rule is fixed where it lives.

## Suggested Fix

Keep the required-binding rule as a *disambiguator* rather than a *filter*: when
more than one candidate route matches the method and path, prefer the one with
the most satisfied required bindings (as today); when exactly one route matches,
resolve to it regardless, and let the operation's boundary validation refuse the
missing member with the protocol's 400. That preserves the S3 behaviour the rule
was written for and removes the need for a per-service `override_operation`.

`vendor/` is read-only to an issue fixer, so this needs a core change.
