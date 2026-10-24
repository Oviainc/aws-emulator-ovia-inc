Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7ebfb9678d6df6b0182d42dae67a3a93fc35190c

# Not-found and already-exists messages are rebuilt from invented templates in the handlers the message-rule fix missed

_Filed: 2026-10-24T10:08:02Z_
_Operation: UpdateAsset | Class: error-response_

> _Note: this issue was identified against the code base at commit `7ebfb9678d6df6b0182d42dae67a3a93fc35190c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This service has one rule for an absent resource — `<Resource> with ID <requested-identifier> not found` — and one for a uniqueness collision — `Another <resource> is already using the same <dimension>`; both live in crate::state (not_found_message / already_using_message) and the control-plane reads (DescribeAsset, DeleteAsset, DescribeAssetModel, DeleteAssetModel, ListAssets, CreateAsset) use them.

Expected: UpdateAsset on an unknown assetId answers `Asset with ID <label> not found`, and UpdateAsset naming an assetExternalId another asset holds answers `Another asset is already using the same externalId` (the dimension that collided), the way CreateAsset already does.

Actual: update_asset.rs builds its own strings — `The asset {label} could not be found.` (line 257) and `The asset {asset_id} already exists.` (line 267), the second naming a resource identity rather than the externalId dimension that actually collided. The same invented templates survive in the other handlers the earlier message-rule fix did not reach: associate_assets.rs:212-253 (asset AND hierarchy: it says 'The hierarchy X could not be found in the asset model Y' where its sibling DisassociateAssets, via state::TripleRefusal, correctly says 'A hierarchy with ID X doesn't exist'), update_asset_property.rs:226-230, disassociate_time_series_from_asset_property.rs:209-213, describe_time_series.rs:288-292, get_asset_property_value.rs:206, get_asset_property_value_history.rs:235, get_asset_property_aggregates.rs:319, get_interpolated_asset_property_values.rs:227, batch_get_asset_property_value.rs:314, batch_get_asset_property_aggregates.rs:400, list_asset_model_properties.rs:339, describe_asset_model_composite_model.rs:287, create_asset_model_composite_model.rs:499 and describe_gateway.rs:197.

## Evidence

state.rs documents the rule it centralises: 'One rule for every not-found path in this service, so no handler invents its own template: AWS states the absence as <Resource> with ID <id> not found' (not_found_message, ~line 1619) and 'AWS names the DIMENSION of the request that collided (externalId, ID)' (already_using_message, ~line 1626). tests/all/hierarchies_etags_and_refusals.rs lines 1212-1262 assert exactly that wording for DescribeAsset/DeleteAsset/DescribeAssetModel/DeleteAssetModel/ListAssets/CreateAsset — and cover none of the handlers listed above, so nothing asserts their divergent text. The same divergence (for the ops that were then covered) was the recorded AWS behavior in the already-fixed issue 20261022T070208Z-iotsitewise-...-6-divergences (Cause 6).

## Steps to Reproduce

1. UpdateAsset with an assetId that does not exist -> 404 with message 'The asset <id> could not be found.' instead of 'Asset with ID <id> not found'.
2. CreateAsset two assets A and B; UpdateAsset on B with assetExternalId = A's externalId -> 409 with 'The asset <A-id> already exists.' instead of 'Another asset is already using the same externalId'.
3. AssociateAssets with a hierarchyId the parent's model does not declare -> 404 'The hierarchy X could not be found in the asset model Y.'; call DisassociateAssets with the same triple -> 404 'A hierarchy with ID X doesn't exist'. The two siblings word the same absence differently.
