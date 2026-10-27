Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# The supplier walk mints a NextToken longer than the 1024 the model allows, so the emulator refuses its own token and the second page is unreachable

_Filed: 2026-10-27T14:25:03Z_
_Operation: ListProcurementPortalSuppliers | Class: pagination_

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListProcurementPortalSuppliers` builds its `NextToken` by hex-encoding a prefix plus the supplier key, and that key is built from CALLER-SUPPLIED identifiers that the model lets run to 1024 characters each (`SupplierIdentifier`, and a `SellerOfRecords` entry). Hex encoding doubles the byte count, so a preference carrying a long-but-legal supplier identifier produces a token well past the `@length(max: 1024)` the model puts on the `NextToken` member — on the OUTPUT member, so the response carries a value the model says that member never holds, and on the INPUT member, so when the client hands that token straight back (the only thing a paginator can do with it) the boundary validator refuses it with a 400 `ValidationException`.

The effect is that the walk cannot be completed: page 1 answers 200 with a token, and every attempt to resume with it is a 400, so the suppliers after the first page are unreadable. The handler's own comment reasons about the token's PREFIX being caller-free and says nothing about the position it appends, which is where all the caller-supplied length enters.

No other listing of this crate has this problem: `ListProcurementPortalPreferences` and `ListProcurementPortals` page on ARNs/portal identifiers the service mints, and `ListInvoiceUnits`/`ListInvoiceSummaries` have a `NextToken` modelled `@length(min: 1)` with no maximum.

## Evidence

Model rule, both sides of the token — `smithy_query(query="operation", operation="ListProcurementPortalSuppliers")`:
  input:  `"NextToken": {"target": "BasicStringWithoutSpace", "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}`
  output: `"NextToken": {"target": "BasicStringWithoutSpace", "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}`

What the handler mints — services/service-com-amazonaws-invoicing-invoicing/src/ops/list_procurement_portal_suppliers.rs:195 and :236:
```
        let token_prefix = format!("{NEXT_TOKEN_PREFIX}{portal_name}:");
...
                next_token = Some(crate::state::encode_page_token(&token_prefix, key));
```
where `key` is the supplier key — src/state.rs:740-747:
```
pub fn procurement_portal_supplier_key(supplier: &ProcurementPortalSupplier) -> String {
    format!(
        "{} {} {}",
        supplier.supplier_identifier,
        supplier.environment.as_deref().unwrap_or_default(),
        supplier.seller_of_record.as_deref().unwrap_or_default(),
    )
}
```
and the encoding doubles it — src/state.rs:1196-1198:
```
pub fn encode_page_token(prefix: &str, position: &str) -> String {
    hex::encode(format!("{prefix}{position}"))
}
```

The lengths that reach that key are caller-supplied and modelled far above the token's own bound — `smithy_query(query="operation", operation="CreateProcurementPortalPreference")`: `"SupplierIdentifier": {"target": "BasicStringWithoutSpace", "required": true, "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}`, and `smithy_query(query="shape", shape="SellerOfRecords")`: `"member": {"target": "BasicStringWithoutSpace", "length": {"min": 0, "max": 1024}, "pattern": "^\\S+$"}`. With the 29-char `NEXT_TOKEN_PREFIX` (list_procurement_portal_suppliers.rs:139 `const NEXT_TOKEN_PREFIX: &str = "procurement-portal-suppliers:";`) plus `SAP_BUSINESS_NETWORK:` the prefix alone is 50 bytes, so any position past ~462 bytes already hex-encodes past 1024.

The handler's comment reasons only about the prefix — list_procurement_portal_suppliers.rs:190-194:
```
        // A token is scoped to the portal it was minted for: ... The RESOLVED
        // portal name namespaces it rather than the request's identifier, so the prefix
        // is one of the enum's values and cannot be made ambiguous by what was asked.
```

Test coverage does not reach it: tests/all/procurement_portal_suppliers.rs:431-459 `pages_concatenate_to_the_whole_listing` walks every page size, but the seeded identifiers are short (e.g. :478 `"SupplierIdentifier": "zip-supplier-max",`), so every minted token stays far inside 1024. That the boundary DOES enforce this operation's input constraints is pinned by tests/all/procurement_portal_suppliers.rs:524-541 `refuses_a_max_results_outside_the_published_range` (a `MaxResults` outside `@range` answers 400 `ValidationException`).

## Steps to Reproduce

1. Start the emulator.
2. POST / with `X-Amz-Target: Invoicing.CreateProcurementPortalPreference` and body `{"ProcurementPortalName":"SAP_BUSINESS_NETWORK","BuyerDomain":"NetworkID","BuyerIdentifier":"AN01","SupplierDomain":"NetworkID","SupplierIdentifier":"<500 non-space characters>","Selector":{"SellerOfRecords":["SOR-A","SOR-B"]},"EinvoiceDeliveryEnabled":false,"PurchaseOrderRetrievalEnabled":false,"Contacts":[{"Name":"n","Email":"n@example.com"}]}` — the 500-character SupplierIdentifier is inside the modelled `@length(max: 1024)`, so the create succeeds and the portal now has two configured suppliers (one per selected seller of record).
3. POST / with `X-Amz-Target: Invoicing.ListProcurementPortalSuppliers` and body `{"PortalIdentifier":"sap-business-network","MaxResults":1}`. Observe HTTP 200 with one supplier and a `NextToken` of more than 1024 characters (2 x (50-byte prefix + ~511-byte key)), past the `@length(max: 1024)` the model puts on that output member.
4. POST / with `X-Amz-Target: Invoicing.ListProcurementPortalSuppliers` and body `{"PortalIdentifier":"sap-business-network","MaxResults":1,"NextToken":"<the token from step 3>"}`. Observe HTTP 400 `ValidationException` ('Member must have length less than or equal to 1024') instead of the second page: the listing cannot be walked past page 1.
5. Repeat steps 2-4 with a short SupplierIdentifier to see the same walk complete normally, confirming the length of the caller-supplied identifier is what breaks it.
