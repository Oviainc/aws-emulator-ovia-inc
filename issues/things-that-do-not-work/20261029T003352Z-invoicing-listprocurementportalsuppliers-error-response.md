Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 52e42c3071caaf4a5ac680f43132e302f2bbfe9a

# The portal 404 splices an unbounded PortalIdentifier into a message the model caps at 1024 characters

_Filed: 2026-10-29T00:33:52Z_
_Operation: ListProcurementPortalSuppliers | Class: error-response_

> _Note: this issue was identified against the code base at commit `52e42c3071caaf4a5ac680f43132e302f2bbfe9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListProcurementPortalSuppliers` is the only operation of this service whose caller-supplied identifier carries NO modelled maximum: `PortalIdentifier` targets `ProcurementPortalIdString`, which declares `@length(min: 1)` and no max. When that identifier names no portal, the handler answers `crate::state::no_such_procurement_portal(&input.portal_identifier)`, which builds the 404's message by splicing the identifier verbatim into "Procurement portal with identifier {identifier} does not exist".

`ResourceNotFoundException$message` targets `BasicString`, modelled `@length(min: 0, max: 1024)`. So any identifier longer than ~977 characters produces a `message` longer than the model says that member ever holds, and a 1 MB identifier produces a ~1 MB refusal body — a small request turned into an arbitrarily large refusal. Only input is validated in the walk (`aws_smithy_schema_server::constraints::validate` is called on the INPUT alone, vendor/smithy-server-runtime/src/operation.rs:412), so nothing clips it before it reaches the client.

The crate already holds exactly this rule in two other places — the pagination codec refuses to let caller-supplied text carry a token past its member's modelled maximum, and the refusal formatter bounds the violation count for precisely the "arbitrarily large refusal" reason — and the 404 helper beside this one (`no_such_procurement_portal_preference`) is safe only because `ProcurementPortalPreferenceArnString` happens to cap at 256. Nothing in the crate (comment, constant or test) states a reason for this path to diverge.

## Evidence

Smithy — the unbounded input, `smithy_query(query="shape", shape="ProcurementPortalIdString")`:
  {"type": "string", "length": {"min": 1}}   ← no maximum
and the bounded member it is written into, `smithy_query(query="shape", shape="ResourceNotFoundException")`:
  "message": {"target": "BasicString", "length": {"min": 0, "max": 1024}, "pattern": "^[\\s\\S]*$"}

The handler that reaches the helper — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_procurement_portal_suppliers.rs:183-189:
```
        let portal_name =
            crate::state::procurement_portal_name_for_identifier(&input.portal_identifier)
                .ok_or_else(|| {
                    ListProcurementPortalSuppliersErrorModeled::ResourceNotFoundException(
                        crate::state::no_such_procurement_portal(&input.portal_identifier),
                    )
                })?;
```

The helper, which splices the identifier with no bound — services/service-com-amazonaws-invoicing-invoicing/src/state.rs:1026-1034:
```
pub fn no_such_procurement_portal(portal_identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{PROCUREMENT_PORTAL_KIND} with identifier {portal_identifier} does not exist"
        )),
        resource_id: None,
        message_key: MessageKey::Capitalised,
    }
}
```

The same rule, stated by this crate for the pagination codec — state.rs:1360-1367:
```
/// A `NextToken` carries the model's constraints on BOTH sides of a paginated operation
/// (the output member a page hands back and the input member the next call hands in), so
/// a token has to stay inside them to be one this service will accept back. Hex doubles
/// the byte count, so `position` must be text whose length THIS SERVICE bounds — an
/// identifier it mints, an enum value, or the ordinal
/// [`encode_page_offset`] encodes — and never a caller-supplied value whose own modelled
/// length can carry the token past the `NextToken` member's maximum.
```

and for the refusal formatter — services/service-com-amazonaws-invoicing-invoicing/src/service/validation.rs:70-75:
```
/// How many violations one refusal states. Each one is rendered TWICE into the body (a
/// clause of `message` and a `fieldList` entry), so an unbounded count turns a small
/// request into an arbitrarily large refusal. The bound is the boundary validator's own
/// (`MAX_VIOLATIONS` in `aws_smithy_schema_server::constraints`), so the two refusal
/// paths read alike.
const MAX_VIOLATIONS: usize = 100;
```

Only the input is validated, so nothing clips the body — vendor/smithy-server-runtime/src/operation.rs:412:
```
        let violations = aws_smithy_schema_server::constraints::validate(&input, O::INPUT_SCHEMA);
```

Test coverage: tests/all/procurement_portal_suppliers.rs:551 pins the 404 for short unknown identifiers only — `for portal_identifier in ["ZIP", "zipp", "sap_business_network", "unknown-portal", "zi"]` — and `grep -n '2048|1024|repeat(' tests/all/procurement_portal_suppliers.rs` matches nothing, so no test asserts or excuses a long identifier.

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.ListProcurementPortalSuppliers` and body `{"PortalIdentifier":"<2000 copies of the letter z>"}` (any value that is not one of `zip`, `sap-business-network`, `jaggaer`, `coupa`).
3. Observe HTTP 404 `ResourceNotFoundException` whose `Message` is "Procurement portal with identifier zzz…z does not exist" — about 2045 characters.
4. Compare with the model: `ResourceNotFoundException$message` targets `BasicString`, `@length(min: 0, max: 1024)`, so the body carries a value the model says that member never holds, and the refusal's size is set by the caller rather than by the service.
