Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6715c947a740417962c3143a65f87ddf3b03a07e

# The ARN-or-bare-UUID identifier resolution rule is written out four times in state.rs, once per resource level

_Filed: 2026-10-28T19:08:01Z_
_Operation: GetIngestionDestination | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6715c947a740417962c3143a65f87ddf3b03a07e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every `Identifier` member of this service accepts exactly two forms (`^arn:.+$` or a bare UUID), and resolving one to a store key is ONE rule: if the identifier is an ARN, find the record whose stored `arn` equals it and answer that record's `id`; otherwise the identifier IS the map key, answered only when the map holds it.

That single rule is implemented four separate times in `src/state.rs`, once per level of the resource chain — `bundle_key` for app bundles, `ingestion_key` for an ingestion inside a bundle, `destination_key` for a destination inside an ingestion, and `app_authorization_key` for an authorization inside a bundle. The four bodies are structurally identical (same `is_arn` branch, same `values().find(|r| r.arn == identifier).map(|r| r.id.clone())`, same `contains_key(identifier).then(|| identifier.to_string())`); only the map and the record type differ. Each copy's own doc comment admits it is a copy ("the same two forms ... accepts", "the same two forms every other `Identifier` in this service accepts"), so the crate already knows this is one rule in four places.

Because there are four copies, any change to how an identifier is matched (trimming, case handling, rejecting an ARN that belongs to another parent, answering a second record that carries the same ARN) has to be made four times, and a change made in three of them leaves one level resolving identifiers differently from its siblings — exactly the drift this crate avoids elsewhere by naming a rule once (`IngestionDestinationRecord::mark_accepted`, `AppAuthorizationRecord::is_oauth2`, `replay_spent_client_token`).

The fix is to express the rule once (a small generic helper over "a map of records that each carry an `id` and an `arn`") and have the four functions call it. No response body and no error changes: the four call sites keep their current signatures and answers.

## Evidence

All four copies live in `services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs`.

Copy 1 — state.rs:841-857:
```
/// The store key of the app bundle `identifier` names.
///
/// The model's `Identifier` shape accepts either form
/// (`^arn:.+$|^[a-f0-9]{8}-…$`), and both name the same resource: an ARN
/// matches the record that owns it, anything else is the bare UUID key.
pub fn bundle_key(&self, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        self.app_bundles
            .values()
            .find(|bundle| bundle.arn == identifier)
            .map(|bundle| bundle.id.clone())
    } else {
        self.app_bundles
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}
```

Copy 2 — state.rs:1149-1164:
```
/// The store key of the ingestion `identifier` names inside `bundle`, by ARN or
/// bare UUID — the same two forms [`ComAmazonawsAppfabricFabricfrontendserviceState::bundle_key`] accepts.
pub fn ingestion_key(bundle: &AppBundleRecord, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        bundle
            .ingestions
            .values()
            .find(|ingestion| ingestion.arn == identifier)
            .map(|ingestion| ingestion.id.clone())
    } else {
        bundle
            .ingestions
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}
```

Copy 3 — state.rs:1166-1182:
```
/// The store key of the ingestion destination `identifier` names inside
/// `ingestion`, by ARN or bare UUID — the same two forms every other
/// `Identifier` in this service accepts.
pub fn destination_key(ingestion: &IngestionRecord, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        ingestion
            .destinations
            .values()
            .find(|destination| destination.arn == identifier)
            .map(|destination| destination.id.clone())
    } else {
        ingestion
            .destinations
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}
```

Copy 4 — state.rs:1184-1200:
```
/// The store key of the app authorization `identifier` names inside `bundle`,
/// by ARN or bare UUID — the same two forms every other `Identifier` in this
/// service accepts.
pub fn app_authorization_key(bundle: &AppBundleRecord, identifier: &str) -> Option<String> {
    if is_arn(identifier) {
        bundle
            .app_authorizations
            .values()
            .find(|authorization| authorization.arn == identifier)
            .map(|authorization| authorization.id.clone())
    } else {
        bundle
            .app_authorizations
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}
```

Three of the four copies are reached from a single handler, `src/ops/get_ingestion_destination.rs:189-201`, via `store.resolve_bundle(&input.app_bundle_identifier)` (→ `bundle_key`), `.resolve_ingestion(&input.ingestion_identifier)` (→ `ingestion_key`) and `.resolve_destination(&input.ingestion_destination_identifier)` (→ `destination_key`).

Existing tests that pin both identifier forms at every level, and must keep passing unchanged:
* `tests/all/app_bundle_ingestion_lifecycle.rs:1179` — `for case in ["by arn", "by uuid"]` in `deleting_an_app_bundle_removes_it_and_its_ingestions` (bundle level);
* `tests/all/ingestion_deletion.rs:164-165` — `("one ingestion, both by uuid", 1, 0, Form::Uuid, Form::Uuid)`, `("two, delete the last by uuid", 2, 1, Form::Arn, Form::Uuid)` (ingestion level);
* `tests/all/ingestion_destination_accepted_status.rs:246` — `let identifier = if by_arn { arn.clone() } else { uuid_of(&arn) };` (destination level);
* `tests/all/update_app_authorization_contract.rs:573-575` — `("both UUIDs", …)`, `("bundle ARN, authorization UUID", …)`, `("bundle UUID, authorization ARN", …)` (authorization level);
* `tests/all/chain_read_not_found_attribution.rs:286-295` — `"GetIngestionDestination with a destination and ingestion a delete removed, by uuid"`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-appfabric-fabricfrontendservice/src/state.rs and read bundle_key (lines 841-857), ingestion_key (1149-1164), destination_key (1166-1182) and app_authorization_key (1184-1200)
2. Observe the four bodies are the same `is_arn` branch over a different map: ARN -> `values().find(|r| r.arn == identifier).map(|r| r.id.clone())`, otherwise `contains_key(identifier).then(|| identifier.to_string())`
3. Observe each copy's doc comment states it repeats the rule the others implement ("the same two forms ... accepts")
4. Change the matching rule in one copy only (for example trim the identifier before comparing) and note that the other three levels now resolve the same identifier differently, with no test or type forcing them back into agreement
