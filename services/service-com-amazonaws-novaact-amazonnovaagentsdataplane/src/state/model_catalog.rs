// SPDX-License-Identifier: CC0-1.0

//! The AI models this service publishes, and what a client of a given compatibility
//! version may do with them.
//!
//! NOTHING IS STORED. The model binds no operation that creates, updates or retires a
//! model — `ListModels` is the only operation that names one — so the catalog is the
//! service's own published fact rather than something a caller writes: a fixed function
//! here answers the same catalog on every call, whoever asks and whenever, which is
//! exactly what a service-managed catalog is. What DOES vary per request is the
//! compatibility view of it ([`usable_at`], [`lowest_usable_compatibility_version`]),
//! because that is a function of the caller's own client compatibility version.
//!
//! This module is the ONE place the catalog's contents live, so every reader of a model
//! id (today `ListModels`, tomorrow anything that resolves an alias) reads one list.

use crate::ops::types::{ModelAlias, ModelLifecycle, ModelSummary};

/// `ModelStatus` `ACTIVE` — the generation generally available for new work.
pub const MODEL_STATUS_ACTIVE: &str = "ACTIVE";

/// `ModelStatus` `LEGACY` — a superseded generation that still runs.
pub const MODEL_STATUS_LEGACY: &str = "LEGACY";

/// `ModelStatus` `DEPRECATED` — a generation that has been retired. It stays in the
/// catalog, so a caller holding its id learns what became of it, and it is offered to no
/// client compatibility version.
pub const MODEL_STATUS_DEPRECATED: &str = "DEPRECATED";

/// `ModelStatus` `PREVIEW` — a generation available ahead of general availability.
pub const MODEL_STATUS_PREVIEW: &str = "PREVIEW";

/// The alias that names the newest generally available model, whichever generation that
/// is in. The per-generation aliases ([`aliases`]) are derived from the catalog's own
/// ids; this one is the catalog-wide stable reference beside them.
pub const LATEST_MODEL_ALIAS: &str = "nova-act-latest";

/// The character that separates a model id's generation from its revision, as this
/// service's ids spell them (`nova-act-v1.2`: revision 2 of generation `nova-act-v1`).
const REVISION_SEPARATOR: char = '.';

/// The models this service publishes: the id, the `ModelStatus` the model is in
/// (`smithy_query(query="shape",shape="ModelStatus")` gives the four values) and the
/// minimum client compatibility version the model can be driven from — "the minimum
/// client compatibility version required to use this model" — oldest revision first, so
/// the LAST entry of a generation is its newest.
///
/// The generations are the `nova-act-v<generation>.<revision>` family this service's
/// model ids name, one revision per lifecycle stage the model declares: a retired first
/// revision, the superseded one that replaced it, the generally available one callers get
/// today, and the next generation in preview, which asks for a newer client than the
/// generation before it.
const PUBLISHED_MODELS: [(&str, &str, i32); 4] = [
    ("nova-act-v1.0", MODEL_STATUS_DEPRECATED, 1),
    ("nova-act-v1.1", MODEL_STATUS_LEGACY, 1),
    ("nova-act-v1.2", MODEL_STATUS_ACTIVE, 1),
    ("nova-act-v2.0", MODEL_STATUS_PREVIEW, 2),
];

/// The published catalog, as `ModelSummary` records in publication order.
pub fn catalog() -> Vec<ModelSummary> {
    PUBLISHED_MODELS
        .iter()
        .map(
            |(model_id, status, minimum_compatibility_version)| ModelSummary {
                model_id: (*model_id).to_string(),
                model_lifecycle: ModelLifecycle {
                    status: (*status).to_string(),
                },
                minimum_compatibility_version: *minimum_compatibility_version,
            },
        )
        .collect()
}

/// Whether a client at `client_compatibility_version` can use `model`.
///
/// Two conditions, one from each member of the record: the client is at least at the
/// model's own `minimumCompatibilityVersion`, and the model's lifecycle still admits use
/// — a `DEPRECATED` model is retired, so no client version reaches it, while `ACTIVE`,
/// `LEGACY` and `PREVIEW` all run.
pub fn usable_at(model: &ModelSummary, client_compatibility_version: i32) -> bool {
    model.model_lifecycle.status != MODEL_STATUS_DEPRECATED
        && model.minimum_compatibility_version <= client_compatibility_version
}

/// The lowest client compatibility version that reaches any model at all, i.e. the
/// version a client below every model's minimum has to reach to use this service.
///
/// `None` only for a catalog whose every entry is retired, which has nothing to
/// recommend.
pub fn lowest_usable_compatibility_version() -> Option<i32> {
    catalog()
        .iter()
        .filter(|model| model.model_lifecycle.status != MODEL_STATUS_DEPRECATED)
        .map(|model| model.minimum_compatibility_version)
        .min()
}

/// The published aliases, each with the model id it currently points at, by alias name.
///
/// An alias is "a stable reference to a model version", so it is derived from the catalog
/// rather than listed beside it: one alias per generation, naming that generation's
/// newest revision that is not retired, plus [`LATEST_MODEL_ALIAS`] naming the newest
/// generally available model of any generation. An alias therefore never points at a
/// retired model, and a new revision moves the alias by being published, which is what
/// makes the reference stable.
///
/// `resolvedModelId` is NOT filled in here: resolution is per caller
/// ([`usable_at`]), so the operation answering a request fills it.
pub fn aliases() -> Vec<ModelAlias> {
    let catalog = catalog();
    // A `BTreeMap` for both reasons: later (newer) revisions of a generation overwrite
    // earlier ones as the catalog is walked in publication order, and the aliases come
    // out in one stable order on every call.
    let mut targets: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for model in &catalog {
        if model.model_lifecycle.status == MODEL_STATUS_DEPRECATED {
            continue;
        }
        targets.insert(
            generation_of(&model.model_id).to_string(),
            model.model_id.clone(),
        );
    }
    if let Some(newest_active) = catalog
        .iter()
        .rev()
        .find(|model| model.model_lifecycle.status == MODEL_STATUS_ACTIVE)
    {
        targets.insert(
            LATEST_MODEL_ALIAS.to_string(),
            newest_active.model_id.clone(),
        );
    }
    targets
        .into_iter()
        .map(|(alias_name, latest_model_id)| ModelAlias {
            alias_name,
            latest_model_id,
            resolved_model_id: None,
        })
        .collect()
}

/// The generation a model id names: everything before its revision.
fn generation_of(model_id: &str) -> &str {
    model_id
        .split_once(REVISION_SEPARATOR)
        .map_or(model_id, |(generation, _)| generation)
}
