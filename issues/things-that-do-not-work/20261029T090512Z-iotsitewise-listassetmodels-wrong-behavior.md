Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: f60279ee1b26e62c04ba7bfaecc753f1127e63e1

# ListAssetModels resolves assetModelVersion=ACTIVE by the model's CURRENT state instead of the crate's one version selector, so a model whose latest write is still settling is dropped although it has an ACTIVE version

_Filed: 2026-10-29T09:05:12Z_
_Operation: ListAssetModels | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `f60279ee1b26e62c04ba7bfaecc753f1127e63e1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListAssetModels is one of the five operations that carry the `assetModelVersion` selector, and it is the only one that does not resolve it through `AssetModelRecord::version_view` — the helper whose own documentation says it is the ONE place every operation carrying the selector resolves it. Instead the handler filters on the model's CURRENT state: `version != VERSION_TYPE_ACTIVE || model.state(now) == STATE_ACTIVE`.

That is not the same rule. A model that was ACTIVE and has since accepted a write reports `UPDATING` until the transition that write owes falls due, and for exactly that window the record keeps `superseded_active` — "The definition of the model's ACTIVE version while the members above are NOT it". `version_view(Some("ACTIVE"))` answers that definition, so DescribeAssetModel, DescribeAssetModelCompositeModel, ListAssetModelProperties and ListAssetModelCompositeModels all report the model's active version during the window. ListAssetModels drops the model from the page entirely: `assetModelVersion=ACTIVE` answers as if the model had no active version at all, which by the crate's own rule is true only of a model no version of which has ever been active (one still being CREATED).

The handler defends the filter with a premise the record contradicts: "This emulator keeps ONE version of an asset model, so ACTIVE resolves to it once the transition the clock owes has fallen due, and to nothing while the model is still settling". The record keeps two definitions while a write settles — that is what `superseded_active` is for — so the comment is stale and the filter implements a rule the rest of the crate does not.

A second symptom follows from the same cause: even in the cases the filter admits, `summary()` reads `record.name`, `record.description`, `record.version` and `record.last_update_date` off the stored (LATEST) definition, so the listing has no path at all for reporting an ACTIVE version's details — which is what the member documents it returns ("The version alias that specifies the latest or active version of the asset model. The details are returned in the response.", the same sentence DescribeAssetModel's selector carries).

## Evidence

The outlier filter and the stale premise it rests on:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/list_asset_models.rs:177-201 —
    "        // `assetModelVersion` selects the LATEST (the documented default) or the
        // ACTIVE version; it carries the modelled pattern `^(LATEST|ACTIVE)$`, which
        // the runtime validates BEFORE this handler runs, so an alias that arrives
        // here is one of those two and nothing else falls through to LATEST.
        // This emulator keeps ONE version of an asset model, so
        // ACTIVE resolves to it once the transition the clock owes has fallen due,
        // and to nothing while the model is still settling ...
        let version = input
            .asset_model_version
            .as_deref()
            .unwrap_or(crate::state::VERSION_TYPE_LATEST);
        ...
            .filter(|model| {
                version != crate::state::VERSION_TYPE_ACTIVE
                    || model.state(now) == crate::state::STATE_ACTIVE
            })"
and the summary it then builds from the stored definition regardless of the selector:
  src/ops/list_asset_models.rs:251-266 — "fn summary(record: &crate::state::AssetModelRecord, now: ::std::time::SystemTime) -> AssetModelSummary { AssetModelSummary { id: record.id.clone(), ... name: record.name.clone(), ... description: record.description.clone().unwrap_or_default(), ... version: Some(record.version.to_string()), } }"

The crate's ONE selector rule, which this handler bypasses:
  src/state.rs:1052-1062 — "This model as the version the selector `version_type` names it as of `now` (`LATEST | ACTIVE`, defaulting to `LATEST`), or `None` when the model has no such version. ... `ACTIVE` is that same definition once the transition the clock owes has fallen due, and the definition the still-settling write superseded until then; a model no version of which has ever been active — one still being created — has no active version at all. ONE place, so every operation carrying the selector, whether as `assetModelVersion` on a read or as `matchForVersionType` on a write's precondition, resolves it identically."
  src/state.rs:1073-1079 — "match version_type.unwrap_or(VERSION_TYPE_LATEST) { VERSION_TYPE_ACTIVE if self.state(now) != STATE_ACTIVE => self.superseded_active.as_ref().map(|definition| Cow::Owned(self.as_of(definition))), _ => Some(Cow::Borrowed(self)), }"

The record member that exists precisely for the window this handler denies:
  src/state.rs:931-944 — "/// The definition of the model's ACTIVE version while the members above are
    /// NOT it: the one the write that is still settling superseded. ... Exactly ONE definition, replaced rather than appended to, so no number of writes grows the record: a model carries its latest definition and at most its active one."

The four siblings that do resolve the selector through it:
  src/ops/describe_asset_model.rs:219-228 — "`assetModelVersion` selects which version of the model to report: the LATEST one (the default) or the ACTIVE one, which is a DIFFERENT definition while a write the model already accepted is still settling. The selector is resolved on the record, so every operation that carries it answers from the same version ..." followed by ".version_view(input.asset_model_version.as_deref(), now)"
  src/ops/list_asset_model_properties.rs:203, src/ops/list_asset_model_composite_models.rs:186-191 ("`version_view`, so this listing and the reads of the model itself answer ..."), src/ops/describe_asset_model_composite_model.rs:215 — the same call.

The member documentation, identical on both operations:
  smithy_query(query="doc", shape="ListAssetModelsRequest", member="assetModelVersion") — "The version alias that specifies the latest or active version of the asset model. The details are returned in the response. The default value is LATEST."

Test coverage: only the never-active case is pinned, and its own comment says so. tests/all/list_asset_models.rs:453-482 (`a_model_still_settling_has_no_active_version_yet`) takes its reading off a single CREATE — "a model whose CREATING -> ACTIVE transition is still owed has no ACTIVE version" — and tests/all/list_asset_models.rs:425-435 asserts only that "every settled model has an ACTIVE version". No test lists under ACTIVE while an UPDATE is settling, which is the case this filter gets wrong; tests/all/asset_model_version_selection.rs:52-66 covers that window for the three reads it names (`Model`, `Properties`, `CompositeModel`) and not for this listing.

## Steps to Reproduce

1. CreateAssetModel M and wait past the activation delay, so M reports ACTIVE.
2. GET /asset-models?assetModelVersion=ACTIVE -> 200, M is listed (version 1).
3. UpdateAssetModel M with a changed assetModelName, so the write is accepted and M reports UPDATING with version 2 while superseded_active holds version 1.
4. Immediately GET /asset-models/{M}?assetModelVersion=ACTIVE -> 200 reporting version 1's definition (the active version still exists).
5. Immediately GET /asset-models?assetModelVersion=ACTIVE.
6. Observed: M is absent from assetModelSummaries, i.e. the listing reports that M has no ACTIVE version, although the describe of the same model at the same instant answers that version's details.
7. Expected per src/state.rs:1052-1062 ("ONE place, so every operation carrying the selector ... resolves it identically") and src/ops/describe_asset_model.rs:219-228: M is listed, with the ACTIVE version's name, description, version and lastUpdateDate — the details the member says are returned.
