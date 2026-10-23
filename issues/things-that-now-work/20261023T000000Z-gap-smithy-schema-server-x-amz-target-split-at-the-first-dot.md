Issue-Severity: HIGH
Issue-Gap-Class: framework

# `vendor/smithy-schema-server` splits `X-Amz-Target` at the FIRST dot, so every awsJson service whose target prefix is a fully qualified shape id is unroutable

_Filed: 2026-10-23T00:00:00Z_
_Operation: vendor/smithy-schema-server (awsJson routing) | Class: framework_

## Description

On the awsJson protocols a request names its operation in the `X-Amz-Target` header as
`<target prefix>.<Operation>`. The vendored router reads it in
`vendor/smithy-schema-server/src/rpc.rs::routing_key_from_x_amz_target`:

```rust
header
    .split_once('.')
    .map(|(_, op)| op)
```

`split_once` cuts at the FIRST dot, so the routing key is everything after the first
segment of the prefix. That is correct only while the prefix carries no dot of its own
(`DynamoDB_20120810.GetItem`, `Logs_20140328.PutLogEvents`).

A number of AWS services publish a FULLY QUALIFIED target prefix — the service's shape id
with its namespace written out. CodeConnections is one:
`com.amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink`. For those,
`split_once` yields `amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink`,
which matches no entry in the service's generated routing table. The router then renders
its routing failure — HTTP 501 `NotImplemented`, "operation
amazonaws.codeconnections.CodeConnections_20231201.DeleteRepositoryLink is not implemented
in this emulator" — for EVERY operation of that service, whether or not the operation has
a handler.

An operation shape name never contains a dot, so the operation is the segment after the
LAST dot, which resolves both prefix forms.

## Observable effect

Against the CodeConnections emulator before the workaround below, every SDK call answered
501 `NotImplemented` naming the dotted key above, including operations whose handler was
fully implemented. The 501 is indistinguishable, to a reader of the response, from a
scaffold stub's own 501 — only the message wording differs ("is not implemented" vs "is
scaffolded but not implemented"), so the gap reads as "nobody has written this operation"
when the real cause is that no handler was reached.

## Scope

Every awsJson1_0 / awsJson1_1 service whose published `X-Amz-Target` prefix contains a
dot. The emulator's own in-process tests do not reveal it, because a hand-built test
request usually carries the short `<ServiceShapeName>.<Operation>` form, which routes.

## Fix (as filed)

In `routing_key_from_x_amz_target`, take the segment after the LAST dot
(`header.rsplit_once('.')`), which is unchanged for a single-segment prefix and correct
for a dotted one. Optionally also verify that the prefix equals the service's shape name
or its fully qualified id, so a foreign prefix is refused rather than routed — the
function has no access to the service identity today, so that part needs the caller to
pass it.

That part cannot be done from inside the emulator repo: `vendor/` is read-only here and
the merge gate refuses a change to it, so the vendored reader still splits at the first
dot. What the emulator CAN hold, and now does, is below.

## Resolution

The rule — on the awsJson protocols the operation is the segment after the target's LAST
dot and the service is the last dot-separated segment of what precedes it, because the
target prefix is opaque SDK text that may carry dots while a Smithy shape name never can
— is stated ONCE for the emulator, in `services/common/src/rpc_target.rs`, and read from
there by both halves of routing:

* `services/common/src/lib.rs`'s shared claim rule (`claims_request`) reads the target's
  SERVICE through `rpc_target::names_service` instead of the framework's first-dot
  `emulator_server::service_from_target`. That half is fixed for EVERY service that
  claims through this crate, not only the dotted ones: before, a service whose published
  prefix carries dots was named `com` by that reader and claimed none of its own traffic.
* the two services whose published prefix is dotted today — CloudTrail
  (`com.amazonaws.cloudtrail.v20131101.CloudTrail_20131101`) and CodeConnections
  (`com.amazonaws.codeconnections.CodeConnections_20231201`) — resolve the OPERATION
  through `rpc_target::requested_operation` in the one seam the framework offers,
  `Service::override_operation`, and recognise their own unsigned traffic through
  `rpc_target::target_operation_of` in `Service::can_handle`. The per-service copies of
  the rule they each carried are gone, and a third such service needs one call, not
  another copy.

The shared reader takes the service identity, so the issue's optional half is in: a
target whose prefix names another service resolves to `None` and gets the protocol's own
routing refusal rather than being routed by its last segment.

Still owed by the core library: `routing_key_from_x_amz_target` resolving the operation
itself, which would let the `override_operation` seams be deleted (the claim half stays
here either way).

## Evidence

`vendor/smithy-schema-server/src/rpc.rs` — `routing_key_from_x_amz_target`'s
`split_once('.')`, whose doc comment states the assumption ("AWS RPC protocols use the
header `x-amz-target: ServiceName.OperationName`. This function returns the substring
after the dot.").
`vendor/server/src/lib.rs` — `service_from_target`'s `target.split('.').next()`, the same
first-dot read on the claim side.
`services/common/src/rpc_target.rs` — the rule, with unit tables over the prefix forms
and both halves of it.
`services/service-com-amazonaws-codeconnections-codeconnections-20231201/tests/all/target_prefix_forms.rs`
— the whole modelled operation surface reached behind five prefix forms, one store behind
all of them, unsigned requests routed by the service their target names, and the targets
that must still be refused.
`services/service-com-amazonaws-cloudtrail-cloudtrail-20131101/tests/all/target_prefix_dispatch.rs`
— the same, for 60 operations under three prefix forms, signed and unsigned.
`services/service-com-amazonaws-codeconnections-codeconnections-20231201/tests/all/repository_link_lifecycle.rs`
— `both_target_prefix_forms_route_to_the_same_operation` drives the whole repository-link
lifecycle under both prefix forms and fails on the fully qualified one without the rule.
