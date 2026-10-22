Issue-Severity: MEDIUM
Issue-Gap-Class: wrong

# The vendored CBOR serializer re-emits a member key inside a list of scalars

_Filed: 2026-10-22T14:10:00Z_
_Operation: vendor/smithy-macros CBOR serialize path | Class: wrong_

## Description

Serializing a structure member whose target is a list of SCALARS through the
generated schema serializer writes the COLLECTION member's key a second time,
inside the array. `vendor/smithy-macros/src/codegen.rs`'s `emit_write` hands the
member's own schema to each element of the list, and the CBOR codec writes that
schema's member name before the element — so an input with
`ProductIds: ["prod-abc1234567890"]` goes out as
`ProductIds: ["ProductIds", "prod-abc1234567890"]`.

The consequence is that a client built from this project's own generated types
cannot send a list-of-scalars member: the emulator refuses its own client's
request, because the first array element is not a value the member admits. A
list of STRUCTURES is unaffected (each element carries its own shape's schema).

This is a SERIALIZE-side defect. The deserialize side reads a plain array
correctly, so a request built by any other client is handled fine, and no
service's response is wrong today — only a body this project serializes itself.

`vendor/` is read-only from inside the emulator, so the fix has to be made in
the core library's codegen: `emit_write` must hand the LIST's member schema (or
the element's own schema) to the element writer rather than the collection
member's.

## Evidence

`services/service-com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement/tests/all/cbor_request.rs`
exists only because of this: the suites there build the `ProductIds` /
`ProductCodes` filter bodies for `ListMarketplaceRevenueShares` directly, as a
CBOR map, instead of through `CborCodec` + `LIST_MARKETPLACE_REVENUE_SHARES_INPUT_SCHEMA`.
That module can be deleted once the codegen is fixed.

## Steps to Reproduce

1. Serialize `ListMarketplaceRevenueSharesInput { catalog: "AWS", product_ids:
   Some(vec!["prod-abc1234567890"]), .. }` with `CborCodec` and that operation's
   input schema.
2. Decode the bytes: the `ProductIds` array holds two text strings, the member
   name and then the value, instead of the one value that was set.
3. Send that body to the emulator: the request is refused, because the member
   name is not a `MarketplaceProductId`.
