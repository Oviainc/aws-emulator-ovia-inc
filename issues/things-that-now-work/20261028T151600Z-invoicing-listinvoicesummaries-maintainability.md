Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2b443a1d32fe9865f4d2b32010552c8e62eefe3f

# Drop the handler-side enum value-set re-check and its hand-copied value sets: this crate's own boundary walk already refuses the same values with the same clause

_Filed: 2026-10-28T15:16:00Z_
_Operation: ListInvoiceSummaries | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2b443a1d32fe9865f4d2b32010552c8e62eefe3f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`list_invoice_summaries.rs` hand-rolls an enum value-set check (`modelled_enum_value`) and applies it to `selector.resourceType` and `filter.receiverRole`, on the premise — stated in its own doc comment — that "the boundary validator enforces the legacy `@enum` string trait only, so an unmodelled value reaches this handler".

That premise is contradicted by this crate's own sibling handler. `UpdateProcurementPortalPreferenceStatus` takes two members targeting the modern enum `ProcurementPortalPreferenceStatus` and performs NO value-set check at all — its only `ValidationException`s are `reason_without_status` and `no_status_stated` — yet the crate's tests pin that a status outside the enum is refused with HTTP 400 `ValidationException` and writes nothing. The refusal therefore comes from the boundary walk this crate runs itself in `service.rs` (`aws_smithy_schema_server::constraints::validate`), which also reaches nested members under exactly the lowerCamel dotted paths the handler hardcodes (`validation_message_wording.rs` pins `rule.linkedAccounts.2.member` and `resourceTags.1.member.key`).

So the same rule — "a member targeting an enum admits only the values that enum declares" — is implemented twice in this crate: once by the boundary walk `FrontDoor` drives for every operation, and once by hand in one handler, together with a hand-copied value set (`LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES`, `RECEIVER_ROLE_VALUES`). The copies can drift: a handler that keeps its own list can accept or refuse a value the boundary does not.

The fix is to delete `modelled_enum_value` and its two call sites. Both paths render through the same formatter (`crate::service::validation::field_validation_failed`), with the same clause, the same `reason` (`fieldValidationFailed`) and the same member path, so every response and every existing test stays unchanged.

## Evidence

The hand-rolled copy and the premise it rests on — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_invoice_summaries.rs:155-182:
```
/// A member that targets an ENUM shape admits only the values that shape declares, and
/// nothing else; a request carrying any other value is refused.
///
/// The check belongs here rather than at the boundary: the generated input type carries
/// such a member as a plain `String`, and the boundary validator enforces the legacy
/// `@enum` string trait only, so an unmodelled value reaches this handler. ...
fn modelled_enum_value(
    path: &str,
    value: &str,
    allowed: &'static [&'static str],
) -> Result<(), ListInvoiceSummariesErrorModeled> {
    if allowed.contains(&value) {
        return Ok(());
    }
    Err(ListInvoiceSummariesErrorModeled::ValidationException(
        crate::service::validation::field_validation_failed(&[Violation {
            path: path.to_string(),
            kind: ViolationKind::EnumValue(allowed.iter().map(|value| (*value).to_string()).collect()),
```
called at :202-206 (`"selector.resourceType"`, `LIST_INVOICE_SUMMARIES_RESOURCE_TYPE_VALUES`) and :259-261 (`"filter.receiverRole"`, `RECEIVER_ROLE_VALUES`).

The sibling that omits the check — src/ops/update_procurement_portal_preference_status.rs:193-203, the ONLY place its two status members (targeting the modern enum `ProcurementPortalPreferenceStatus`, `smithy_query(query="shape", shape="ProcurementPortalPreferenceStatus")` → `"type": "enum"`) are inspected:
```
fn stated_status(
    status: Option<String>,
    reason: Option<String>,
    members: (&str, &str),
) -> Result<Option<(String, Option<String>)>, ValidationException> {
    match status {
        Some(status) => Ok(Some((status, reason))),
        None if reason.is_some() => Err(reason_without_status(members)),
        None => Ok(None),
    }
}
```
— no value-set test; the status is then written straight into state at :352-358. `PROCUREMENT_PORTAL_PREFERENCE_STATUS_VALUES` exists (src/ops/types/shared_types.rs:768) and is referenced only from a doc comment (state.rs:631).

Yet the boundary refuses it — tests/all/procurement_portal_preference_status.rs:539-580:
```
/// RULE: which statuses a plane may hold is the modelled enum's value set, so a value
/// outside it is refused and writes nothing.
#[tokio::test]
async fn a_status_outside_the_modelled_enum_is_refused() {
    ...
    for (index, status) in ["active", "Validated", "NOT_A_STATUS", ""]
    ...
            assert_eq!(
                http_status, 400,
```
(the same row is pinned again in tests/all/refused_procurement_portal_writes.rs:322-329, "a status update naming a status outside the modelled enum" → 400 `ValidationException`).

The boundary walk that answers it, in this crate — src/service.rs:261-265:
```
                    // A built input: every modelled constraint, in one walk.
                    Ok(input) => {
                        let violations =
                            aws_smithy_schema_server::constraints::validate(&input, schema);
                        return (!violations.is_empty()).then_some(violations);
```
reached for every implemented operation, `ListInvoiceSummaries` included (service.rs:182), and rendered through the same formatter the handler uses (service.rs:306 `validation::field_validation_failed(&violations)`).

That walk reaches NESTED members with the exact paths the handler hardcodes — tests/all/validation_message_wording.rs:347-377:
```
        // A violation on a LIST ELEMENT inside a structure member: the path reaches it,
        ...
            vec![Expected::present(
                "rule.linkedAccounts.2.member",
...
            vec![Expected::present(
                "resourceTags.1.member.key",
```

Tests that cover the behaviour and must stay green after the removal: tests/all/invoice_summaries.rs:552-599 (`an_unmodelled_enum_value_is_refused`, asserting 400 + `ValidationException` + the message naming `selector.resourceType` / `filter.receiverRole`) and tests/all/procurement_portal_preference_status.rs:545.

## Steps to Reproduce

1. Read src/ops/list_invoice_summaries.rs:155-182 — the hand-rolled `modelled_enum_value` and its claim that the boundary validator "enforces the legacy `@enum` string trait only".
2. Read src/ops/update_procurement_portal_preference_status.rs:193-203 and :267-377 — a sibling handler whose two members target the modern enum `ProcurementPortalPreferenceStatus` and which performs no value-set check anywhere.
3. Read tests/all/procurement_portal_preference_status.rs:539-580 — the crate's own test pins HTTP 400 `ValidationException` for `"NOT_A_STATUS"` on both planes, which only the boundary walk can be answering.
4. Conclude the handler-side check in list_invoice_summaries.rs is a second implementation of a rule src/service.rs:261-265 already applies to every operation; delete it and its two call sites and confirm tests/all/invoice_summaries.rs:554 still passes unchanged.
