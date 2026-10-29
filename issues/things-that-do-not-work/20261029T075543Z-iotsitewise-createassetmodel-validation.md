Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 97cefcb07ab7801eeb37fff053a06a3892425e6e

# assetModelName is documented as "A unique name for the asset model" but a second asset model may take a name already in use, although the crate enforces the identical rule for gatewayName

_Filed: 2026-10-29T07:55:43Z_
_Operation: CreateAssetModel | Class: validation_

> _Note: this issue was identified against the code base at commit `97cefcb07ab7801eeb37fff053a06a3892425e6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateAssetModel enforces uniqueness on the two identifiers whose docs say so — a caller-supplied `assetModelId` ("it must be globally unique") and `assetModelExternalId` ("must be unique within your Amazon Web Services account") — and refuses each with the modelled `ResourceAlreadyExistsException`. It does not look at `assetModelName` at all, although that member's documentation is just as explicit: "A unique name for the asset model." Two models created with the same name are both stored, so the account ends up holding several asset models the caller can only tell apart by id, and the modelled 409 for a collision is never reached on that dimension.

This is not a rule the crate declines to implement: it implements exactly this rule, from exactly this wording, for gateways. CreateGateway refuses a second gateway taking a name in use, quoting "`gatewayName` is \"A unique name for the gateway\"", and UpdateGateway applies the same check to a rename, calling it "the same one-name-one-gateway rule `CreateGateway` enforces". The crate even states the inverse case deliberately: a test explains that two portals MAY share a name because "`portalName` is \"A friendly name for the portal\" — not a unique one, unlike `gatewayName`". `assetModelName` falls on the gateway side of that distinction and is treated as if it fell on the portal side. (UpdateAssetModel has the same hole for a rename; it checks only `assetModelExternalId`, src/ops/update_asset_model.rs:340.)

## Evidence

The model sentence, smithy_query(query="doc", shape="CreateAssetModelRequest", member="assetModelName"): "A unique name for the asset model." — and the operation models the collision error: errors include {"shape": "ResourceAlreadyExistsException", "httpError": 409}.

The same sentence restated in the handler's own contract header, services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/create_asset_model.rs:84-85:
    "//! * `assetModelName` (Name, len 1..256, pattern ^[^\u0000-\u001F\u007F]+$) **required** — A
     //!   unique name for the asset model."

What the handler actually enforces — id and externalId only, src/ops/create_asset_model.rs:319-342:
    "        // A caller-supplied id must be globally unique; an absent one is minted.
        let asset_model_id = match input.asset_model_id.clone() { Some(id) => { if store.asset_models.contains_key(&id) { return Err(already_exists(&region, &id, \"ID\")); } ...
        // `assetModelExternalId` must be unique within the account. ...
        if let Some(external) = input.asset_model_external_id.as_deref() { if let Some(existing) = store.asset_model_external_ids.get(external).cloned() { return Err(already_exists(&region, &existing, \"externalId\")); } }"
  and the stored record takes the name unchecked, src/ops/create_asset_model.rs:399 — "name: input.asset_model_name.clone(),". A grep for `asset_model_name` in that file finds only the header line, the fingerprint and this write — no uniqueness test.

The sibling of this crate that implements the identical documented rule:
  src/ops/create_gateway.rs:267-275 — "// `gatewayName` is \"A unique name for the gateway\", so a second gateway may not take a name already in use." ... "if let Some(existing) = store.gateways.values().find(|record| record.name == input.gateway_name) { return Err(already_exists(existing)); }"
  src/ops/update_gateway.rs:190-202 — "// `gatewayName` is \"A unique name for the gateway\", so a rename may not take a name another gateway already holds — the same one-name-one-gateway rule `CreateGateway` enforces".
And the crate's own statement of where the rule does NOT apply, tests/all/portal_lifecycle.rs:550-556 — "async fn two_portals_may_share_a_name_and_stay_separate_resources() { ... // `portalName` is \"A friendly name for the portal\" — not a unique one, unlike `gatewayName` — so a second portal under the same name is a second resource with an identity of its own rather than a conflict." (CreateAsset is on that side too: `assetName` is documented "A friendly name for the asset.")

Test coverage: the gateway refusal is pinned (tests/all/gateway_lifecycle.rs:422-453 — "Same name, a different device each time: the name alone is the conflict." ... "a duplicate name expected 409, got {status} {conflict}"). Nothing pins duplicate asset model names as ALLOWED on purpose — there is no `two_asset_models_may_share_a_name` case anywhere in tests/. One suite does reuse a name incidentally: tests/all/hierarchies_etags_and_refusals.rs:561-622 creates five models named "Alpha" while varying description/properties/hierarchies, because its subject is ETag distinctness ("Rows differ along what the ETag generalises over: a model's name, its description, how many properties it carries, and whether it carries a hierarchy"); those rows need distinct names for the fix to land, which does not change what that suite asserts.

## Steps to Reproduce

1. POST /asset-models with {"assetModelName": "Boiler"} -> 202, model M1.
2. POST /asset-models with {"assetModelName": "Boiler"} again (no assetModelId, no assetModelExternalId).
3. Observed: 202 and a second model M2; GET /asset-models lists two models whose assetModelName is "Boiler".
4. Expected, per the member's documentation ("A unique name for the asset model.") and the identical rule this crate enforces for gatewayName (src/ops/create_gateway.rs:267-275, pinned by tests/all/gateway_lifecycle.rs:453): the second create is refused with the modelled ResourceAlreadyExistsException (409) naming the model that already holds the name, and nothing is stored.
5. The same reuse through PUT /asset-models/{M2} with {"assetModelName": "Boiler"} renames M2 onto M1's name and is likewise accepted (src/ops/update_asset_model.rs:340 checks only externalId), where UpdateGateway refuses the equivalent rename (src/ops/update_gateway.rs:195-202).
