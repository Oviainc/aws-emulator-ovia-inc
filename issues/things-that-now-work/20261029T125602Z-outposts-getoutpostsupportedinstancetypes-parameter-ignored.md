Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: dbe80abdd81a79f1a95892fbee40e178463ee847

# GetOutpostSupportedInstanceTypes resolves OrderId and then drops it, on two premises the crate's own catalog code contradicts (and which contradict each other)

_Filed: 2026-10-29T12:56:02Z_
_Operation: GetOutpostSupportedInstanceTypes | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `dbe80abdd81a79f1a95892fbee40e178463ee847`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetOutpostSupportedInstanceTypes` takes an `OrderId` query member, resolves it against the store (404 when it names no order, or an order of another Outpost), and then never uses it again: the page is cut from `store.orderable_instance_types` filtered only by the Outpost's `SupportedHardwareType` and, when the request also named one, the asset's `ComputeAttributes`. `order_id` appears nowhere after the existence check, so the answer for `?OrderId=oo-A` and `?OrderId=oo-B` on one Outpost is byte-for-byte identical even when the two orders bought different catalog configurations.

The handler does not drop it silently by accident — it states two reasons, and BOTH are false in this crate, and the two disagree with each other:

1. The module header says the order cannot narrow the report because "this emulator has no catalog behind those SKUs". The crate DOES have a catalog behind those SKUs: `ComAmazonawsOutpostsOutpostsolafserviceState::catalog_item` resolves a `CatalogItemId` to a published `CatalogItemRecord`, and `GetCatalogItem` / `ListCatalogItems` publish exactly those records.
2. The body comment says the published catalog "reports which instance FAMILIES a configuration carries capacity of rather than the instance types". The crate's own `ordered_instance_type_names` does precisely the derivation the comment calls impossible: it walks an Outpost's orders, resolves each line item's `CatalogItemId` through `catalog_item`, and pushes `EC2Capacity$MaxSize` — an INSTANCE TYPE, not a family — as a reported instance-type name. `ec2_capacities` is documented as "one entry per instance FAMILY the hardware runs, naming the largest size of that family", i.e. it carries the instance type for each family.

So the one helper that would honour the `OrderId` scope already exists in this crate and is already used by `GetOutpostInstanceTypes`'s configured-instance-type report; this handler's own justification for not calling it is a premise the same crate refutes in two places. The result is a documented input member that costs a 404 when wrong and changes nothing when right.

## Evidence

The premise in the handler's module header — `src/ops/get_outpost_supported_instance_types.rs:31-38`:

    //! An `OrderId` names an Amazon Web Services Outposts ORDER, which scopes the report
    //! to the hardware that order is for: an order this store holds — `CreateOrder`
    //! persists one against the Outpost it names — has to be an order of THIS Outpost,
    //! and one that is not, or one the store holds no record of at all, is answered with
    //! the not-found error this operation models, exactly as an `OutpostIdentifier` the
    //! store does not hold is. An order of this Outpost narrows the report by nothing: what
    //! it would narrow by is the catalog items its line items name, and this emulator has
    //! no catalog behind those SKUs.

The DIFFERENT premise the body of the same file gives for the same decision — `src/ops/get_outpost_supported_instance_types.rs:249-256`:

        // An order scopes the report to the hardware THAT order is for, so it has to
        // be an order of this Outpost: one this store holds no record of, or one
        // placed against another Outpost, names no hardware here and is the same
        // not-found error, echoed on the member it arrived in. An order it does hold
        // narrows nothing — what an order would narrow by is the catalog items its
        // line items name, and the published catalog reports which instance FAMILIES
        // a configuration carries capacity of rather than the instance types the
        // hardware of one order supports — so the report is the Outpost's own.

The order is resolved and then never mentioned again — `src/ops/get_outpost_supported_instance_types.rs:259-262` is the last use of `input.order_id`:

        if let Some(order_id) = input.order_id.as_deref().filter(|id| !id.is_empty()) {
            let places_this_outpost = store
                .order(&region, order_id)
                .is_some_and(|record| record.order.outpost_id.as_deref() == Some(&outpost_id));

and the page is built with no reference to it — `src/ops/get_outpost_supported_instance_types.rs:304-310`:

        let (instance_types, next_token) = crate::state::page(
            crate::state::SUPPORTED_INSTANCE_TYPE_PAGE,
            store
                .orderable_instance_types
                .iter()
                .filter(|entry| supports(&entry.instance_type, form_factor, hardware))

The catalog the header says does not exist — `src/state.rs:3156-3163`, the helper `src/ops/get_catalog_item.rs:158-160` calls to publish an item:

    /// The hardware configuration this service publishes under the `SkuCode`
    /// `catalog_item_id`, or `None` for a code of that form it publishes nothing
    /// under — which this service holds no published hardware facts for.
    pub fn catalog_item(&self, catalog_item_id: &str) -> Option<&CatalogItemRecord> {

and the crate's own per-order derivation of INSTANCE TYPES from "the catalog items its line items name", which the body comment says is not available — `src/state.rs:3336-3367`:

    /// The instance types the hardware the Outpost `outpost_id` names in `region`
    /// arrived carrying: the largest size of each instance family the configurations
    /// its uncancelled orders subscribe to publish EC2 capacity of, in the order those
    /// orders name them. See [`Self::configured_instance_types`].
    fn ordered_instance_type_names(&self, region: &str, outpost_id: &str) -> Vec<String> {
        ...
        for record in orders {
            for item in record.order.line_items.iter().flatten() {
                let Some(catalog_item) = item
                    .catalog_item_id
                    .as_deref()
                    .and_then(|catalog_item_id| self.catalog_item(catalog_item_id))
                else {
                    continue;
                };
                for capacity in catalog_item.item.ec2_capacities.iter().flatten() {
                    if let Some(max_size) = capacity.max_size.as_deref() {
                        push_once(&mut names, max_size);
                    }
                }

and the shape that walk reads, which carries the instance TYPE per family — `src/state.rs:2312-2314`:

    /// The EC2 capacity one configuration carries: one entry per instance FAMILY the
    /// hardware runs, naming the largest size of that family and how many of that size
    /// the configuration's compute capacity provides.

Test coverage today: `tests/all/supported_instance_types.rs` is the suite for this operation and `tests/all/configured_instance_types.rs` is the suite that exercises `ordered_instance_type_names` through `GetOutpostInstanceTypes`; no test asserts that two different orders on one Outpost are answered differently, so neither symptom is pinned with a stated reason.

## Steps to Reproduce

1. POST /sites with {"Name":"s"}; POST /outposts with that SiteId and {"Name":"o","SupportedHardwareType":"RACK"}.
2. GET /catalog/items and pick two published RACK items whose Ec2Capacities name different families/MaxSize values.
3. POST /orders twice on that Outpost, order A with a LineItem for the first SKU and order B with a LineItem for the second; note OrderId A and OrderId B.
4. Wait for both orders to be fulfilled so settle_outposts installs the Outpost's hardware.
5. GET /outposts/{OutpostIdentifier}/supportedInstanceTypes?OrderId=<A> and GET /outposts/{OutpostIdentifier}/supportedInstanceTypes?OrderId=<B> — both answer the identical list (the whole RACK catalog), and the identical list the call with no OrderId answers, so the member changed nothing.
6. GET /outposts/{OutpostIdentifier}/instanceTypes — the sibling report DOES derive instance types from those same line items through state::catalog_item / EC2Capacity$MaxSize, showing the catalog the OrderId branch says does not exist.
