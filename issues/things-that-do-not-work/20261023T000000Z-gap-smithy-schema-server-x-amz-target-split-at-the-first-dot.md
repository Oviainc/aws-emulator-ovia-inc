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

## Fix

In `routing_key_from_x_amz_target`, take the segment after the LAST dot
(`header.rsplit_once('.')`), which is unchanged for a single-segment prefix and correct
for a dotted one. Optionally also verify that the prefix equals the service's shape name
or its fully qualified id, so a foreign prefix is refused rather than routed — the
function has no access to the service identity today, so that part needs the caller to
pass it.

This cannot be done from inside the emulator repo: `vendor/` is read-only here and the
merge gate refuses a change to it, so it needs a change to the vendored core library.

## Current workaround, and why it should be removed

`services/service-com-amazonaws-codeconnections-codeconnections-20231201/src/service.rs`
works around it in the one seam the framework offers, `Service::override_operation`: it
takes the segment after the last dot of the routing key and looks that up in the
generated routing table, declining to `None` for a key that names no modelled operation
so the protocol-correct routing refusal still stands. That is a per-service copy of a
router rule, and every other affected awsJson service has to repeat it. It should be
deleted once `routing_key_from_x_amz_target` resolves the operation itself.

## Evidence

`vendor/smithy-schema-server/src/rpc.rs` — `routing_key_from_x_amz_target`'s
`split_once('.')`, whose doc comment states the assumption ("AWS RPC protocols use the
header `x-amz-target: ServiceName.OperationName`. This function returns the substring
after the dot.").
`services/service-com-amazonaws-codeconnections-codeconnections-20231201/tests/all/repository_link_lifecycle.rs`
— `both_target_prefix_forms_route_to_the_same_operation` drives the whole repository-link
lifecycle under both prefix forms and fails on the fully qualified one without the
workaround.
