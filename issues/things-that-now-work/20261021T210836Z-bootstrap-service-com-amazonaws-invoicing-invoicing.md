Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the Invoicing emulator

_Filed: 2026-10-21T21:08:36Z_
_Operation: service-com-amazonaws-invoicing-invoicing | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 21 operations in `services/service-com-amazonaws-invoicing-invoicing/src/ops/` is an unmodified scaffold that answers HTTP 501 `NotImplemented` and holds no state. Implement a FIRST, SMALL stateful slice: the invoice-unit lifecycle, which is the service's primary resource.

Happy path to implement, in order (wire protocol `awsJson1_0`, `POST /` with `X-Amz-Target: Invoicing.<OperationName>`):

1. **CreateInvoiceUnit** — input `Name` (required, 1..50, pattern `^(?! )[\p{L}\p{N}\p{Z}-_]*(?<! )$`), `InvoiceReceiver` (required, `^\d{12}$`), `Description` (0..500), `TaxInheritanceDisabled` (default false), `Rule` (required, `InvoiceUnitRule{LinkedAccounts, BillSourceAccounts}`), `ResourceTags` (list of `ResourceTag{Key,Value}`), `ClientToken`. Must MINT and persist a new invoice unit keyed by a generated ARN of the modeled form `arn:aws:invoicing::<12-digit-account>:invoice-unit/<id>` (must satisfy the output pattern `^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:[-a-zA-Z0-9/:_]+$`), record `LastModified` = now, and return `InvoiceUnitArn`. The operation is `@idempotent`: a repeat with the same `ClientToken` must return the SAME ARN, not create a second unit.
2. **GetInvoiceUnit** — input `InvoiceUnitArn` (required), optional `AsOf`. Must look the unit up in state and return the stored `InvoiceUnitArn`, `InvoiceReceiver`, `Name`, `Description`, `TaxInheritanceDisabled`, `Rule`, `LastModified` — exactly what CreateInvoiceUnit stored. Unknown ARN -> `ResourceNotFoundException` (HTTP 404).
3. **ListInvoiceUnits** — input `Filters` (`Names`, `InvoiceReceivers`, `Accounts`, `BillSourceAccounts`; multiple filters AND-ed, values within one filter OR-ed), `MaxResults` (1..500, default 500), `NextToken`, `AsOf`. Must return the `InvoiceUnits` actually in state (each a full `InvoiceUnit` structure), honour `MaxResults` as a page size and return/accept an opaque `NextToken` so a second page continues where the first stopped, and omit `NextToken` on the last page.
4. **UpdateInvoiceUnit** — input `InvoiceUnitArn` (required), `Description`, `TaxInheritanceDisabled`, `Rule`, `ClientToken`. Must mutate the STORED unit in place (only the members present in the request), bump `LastModified`, and return `InvoiceUnitArn`; `Name` and `InvoiceReceiver` are immutable. A following GetInvoiceUnit must show the new values. Unknown ARN -> `ResourceNotFoundException` (404).
5. **DeleteInvoiceUnit** — input `InvoiceUnitArn` (required), `ClientToken`. Must remove the unit from state and return `InvoiceUnitArn`. A following GetInvoiceUnit -> `ResourceNotFoundException` (404) and ListInvoiceUnits must no longer include it.
6. **ListTagsForResource** — input `ResourceArn` (required, pattern `^arn:aws[-a-z0-9]*:(invoicing)::[0-9]{12}:[-a-zA-Z0-9/:_]+$`). Must return the `ResourceTags` persisted by CreateInvoiceUnit's `ResourceTags`, and `ResourceNotFoundException` for an ARN not in state. (TagResource/UntagResource may stay stubbed in this slice.)

NEVER STUBS: each of these must compute its response from persisted state. Hardcoded ARNs, echoing the request back, or generating an ARN without storing the unit are failures — a create followed by a get/list must return what was created, and a delete must be observable by the next call.

## State Model

In `src/state.rs` (`ComAmazonawsInvoicingInvoicingState`, reached via `crate::state::service_state(&*self.engine)` so all handlers share one store):

- `invoice_units: BTreeMap<String /* InvoiceUnitArn */, InvoiceUnit>` where
  `InvoiceUnit { arn: String, name: String, invoice_receiver: String, description: Option<String>, tax_inheritance_disabled: bool, rule: InvoiceUnitRule { linked_accounts: Vec<String>, bill_source_accounts: Vec<String> }, last_modified: SystemTime }`
- `invoice_unit_names: BTreeSet<String>` (or derived) to enforce name uniqueness per account — duplicate `Name` on create -> `ValidationException`.
- `resource_tags: BTreeMap<String /* ResourceArn */, BTreeMap<String, String>>` backing `ResourceTags` / `ListTagsForResource`.
- `idempotency_tokens: BTreeMap<String /* ClientToken */, String /* resulting ARN */>` so `@idempotent` create/update/delete replays return the first result instead of duplicating state.
- `next_invoice_unit_id: u64` (or a UUID source) for ARN minting, plus the emulator's account id used in the ARN.

BTreeMap keeps list output in a stable, deterministic order so pagination tokens are meaningful.

Out of scope for this first slice (stay 501, filed separately later): BatchGetInvoiceProfile, GetInvoicePDF, ListInvoiceSummaries, TagResource, UntagResource, and the whole ProcurementPortalPreference family (Create/Put/Get/Delete/List, Send/VerifyProcurementPortalValidation, UpdateProcurementPortalPreferenceStatus, ListProcurementPortals, ListProcurementPortalSuppliers).

## Evidence

`services/service-com-amazonaws-invoicing-invoicing/OPERATIONS.md` lists all 21 operations with the input/output members and errors cited above (CreateInvoiceUnit lines 34-39, DeleteInvoiceUnit 48-53, GetInvoiceUnit 69-74, ListInvoiceUnits 89-94, ListTagsForResource 117-122, UpdateInvoiceUnit 150-155) and documents the `awsJson1_0` / `X-Amz-Target: Invoicing.<Op>` wire protocol. `smithy_query shape=InvoiceUnit,InvoiceUnitRule,ResourceTag,Filters` gives the stored record's members (`InvoiceUnitArn`, `InvoiceReceiver`, `Name`, `Description`, `TaxInheritanceDisabled`, `Rule`, `LastModified`), the rule's `LinkedAccounts`/`BillSourceAccounts`, the tag's required `Key`/`Value`, and the AND/OR filter semantics for ListInvoiceUnits. `grep '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/` matches all 21 handler files, so nothing is implemented and no test asserts any behavior yet.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `invoicing` client at it.
2. CreateInvoiceUnit with Name="BootstrapUnit", InvoiceReceiver="123456789012", Description="first slice", TaxInheritanceDisabled=false, Rule={LinkedAccounts:["210987654321"]}, ResourceTags=[{Key:"env",Value:"test"}], ClientToken="tok-1" — expect 200 with an InvoiceUnitArn matching the modeled ARN pattern (currently HTTP 501).
3. Repeat the identical CreateInvoiceUnit with ClientToken="tok-1" — expect the SAME InvoiceUnitArn and still exactly one unit in state.
4. GetInvoiceUnit with that ARN — expect Name/InvoiceReceiver/Description/TaxInheritanceDisabled/Rule to match what was created, plus a LastModified timestamp.
5. ListInvoiceUnits with no filters — expect the unit present; with Filters={Names:["BootstrapUnit"]} expect it; with Filters={Names:["nope"]} expect an empty list.
6. Create two more units, then ListInvoiceUnits with MaxResults=2 — expect 2 items plus a NextToken, and the follow-up call with that NextToken to return the remaining items with no NextToken and no duplicates.
7. ListTagsForResource with the unit ARN — expect ResourceTags=[{Key:"env",Value:"test"}].
8. UpdateInvoiceUnit with the ARN, Description="updated", Rule={LinkedAccounts:["210987654321","310987654321"]} — then GetInvoiceUnit must show the new description and rule and a newer LastModified.
9. DeleteInvoiceUnit with the ARN — expect 200 echoing the ARN; then GetInvoiceUnit on it must return ResourceNotFoundException (HTTP 404) and ListInvoiceUnits must not include it.
10. GetInvoiceUnit with an ARN that was never created — expect ResourceNotFoundException (HTTP 404), not 501 or 200.
