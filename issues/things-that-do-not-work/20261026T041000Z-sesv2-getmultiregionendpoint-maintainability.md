Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: b08cdb5f6e4f16c8cb7cfab1e60e83a5ca2e511d

# Factor the one "error document keeps its message member" protocol wrapper three services implement separately into a shared helper

_Filed: 2026-10-26T04:10:00Z_
_Operation: GetMultiRegionEndpoint | Class: maintainability_

> _Note: this issue was identified against the code base at commit `b08cdb5f6e4f16c8cb7cfab1e60e83a5ca2e511d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three service crates implement the SAME wire rule — a `restJson1` error document carries the
error shape's `message` member even when the service supplies no message, written as JSON
`null` rather than dropped, so a client can tell "no message was supplied" from "an empty
message was supplied" — and each implements it with its OWN copy of the same three types:

* `services/service-com-amazonaws-polly-parrot-v1/src/service/wire.rs`
* `services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/service/wire.rs`
* `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/service/wire.rs`

Each copy is the same `message_member(error_schema)` lookup (a plain string body member named
`message`/`Message` with no HTTP binding), the same `MessageAlwaysPresent` wrapper that writes
the member as null when the inner structure wrote nothing for it, the same `TrackMessage`
serializer whose ~20 `write_*` methods do nothing but note the member and delegate, and the
same `ServerProtocol` impl that delegates every method to `RestJsonServerProtocol` except
`serialize_error`. The duplicated span is mechanical delegation: a `ShapeSerializer` method
added to the framework has to be mirrored in three places, and a copy that is missed stops
tracking the member silently.

`services/common` is already the shared home for cross-service wire concerns of exactly this
kind (`services/common/src/response_media_type.rs` re-announces a service's media type through
the same `Service::override_protocol` seam, `services/common/src/validation.rs` hosts the
shared validating serializer). The rule both copies implement is one rule, so one
`services/common` helper — the protocol wrapper, parameterised by the inner protocol or
constructed for `restJson1` — can serve all three.

The refactor changes NO response: every service's `serialize_error` already produces the same
document for an error that supplies a message, and the same member-present-as-null document for
one that does not. It must keep each service's `service.rs` returning its protocol from
`override_protocol` and must not touch any existing test.

## Evidence

* `services/service-com-amazonaws-polly-parrot-v1/src/service/wire.rs` — `message_member`,
  `MessageAlwaysPresent`, `TrackMessage`, `NullMessageRestJson` (the whole file), wired in at
  `src/service.rs` (`protocol: wire::NullMessageRestJson`, `override_protocol`).
* `services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/service/wire.rs` — the
  same types, same shape.
* `services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/service/wire.rs` — the third
  copy, added for `GetMultiRegionEndpoint`'s message-less `NotFoundException`.
* `services/common/src/response_media_type.rs` — the precedent for hosting an
  `override_protocol` wrapper shared by several services.

## Steps to Reproduce

1. Open the three `src/service/wire.rs` files above side by side: `message_member`,
   `MessageAlwaysPresent::serialize_members`, `TrackMessage` and the `ServerProtocol` impl are
   the same code in each.
2. Add a method to the framework's `ShapeSerializer` trait and note that three separate
   `TrackMessage` impls must each mirror it for the member tracking to stay correct.
3. Note that `services/common` already hosts an `override_protocol` wrapper several services
   share, so the shared home for this one exists and is unused.
