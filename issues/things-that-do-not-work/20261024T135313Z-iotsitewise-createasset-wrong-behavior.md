Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 30fa86cd244b801d1c6064a0a321e2bf32f37967

# An attribute property's model defaultValue is never materialised, so a new asset's attributes have no current value

_Filed: 2026-10-24T13:53:13Z_
_Operation: CreateAsset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `30fa86cd244b801d1c6064a0a321e2bf32f37967`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Expected: an `attribute` property defined with `type.attribute.defaultValue` is initialised to that value on every asset created from the model, so immediately after `CreateAsset` a read of that property reports a value — `GetAssetPropertyValue` returns the default (quality GOOD, timestamp at asset creation) and `BatchGetAssetPropertyValue` returns an `assetPropertyValue` for the entry. Only a later write overrides it.

Actual: `create_asset.rs` materialises each model property definition into an `AssetProperty` record (id/name/dataType/unit) but seeds no sample into `property_values`, and nothing else derives a value from the definition's `defaultValue`. A freshly created asset's attribute property therefore has an empty series, so `GetAssetPropertyValue` answers 200 with no `propertyValue` and the batch read answers a success entry with no value — the default declared on the model is observable only as part of the property *type* (through DescribeAssetProperty), never as the property's current value.

## Evidence

Smithy model: `Attribute.defaultValue` is the attribute's value for assets created from the model (`PropertyType.attribute`), and `DescribeAssetProperty`'s own doc states that updating the default value in the model is reflected in the response — i.e. the default is the attribute's value, not merely documentation. Well-established behaviour: SiteWise initialises an asset's attribute to the model's default at creation.

Handler: `src/ops/create_asset.rs:253-270` builds the `AssetProperty` list from `model.properties` and never touches `store.property_values`; `src/ops/get_asset_property_value.rs:222-224` then reports `store.series(&stream).last()`, which is empty.

Tests: `tests/all/batch_get_latest_values.rs:73` defines `Serial` as an attribute with `defaultValue: "unset"` and lines 261-266 assert the opposite of AWS — that the property "nothing was written to" has NO `assetPropertyValue`; that assertion encodes the current emulator behaviour and would need to change with the fix.

## Steps to Reproduce

1. CreateAssetModel with a property `Serial` of dataType STRING and `type: {attribute: {defaultValue: "unset"}}`.
2. CreateAsset from that model; note the materialised property id from DescribeAsset.
3. GetAssetPropertyValue for (assetId, that propertyId) -> 200 with no `propertyValue`; AWS reports `{value:{stringValue:"unset"}, quality:"GOOD", timestamp:<asset creation>}`.
4. BatchGetAssetPropertyValue for the same entry -> a success entry with no `assetPropertyValue`, where AWS carries the default value.
