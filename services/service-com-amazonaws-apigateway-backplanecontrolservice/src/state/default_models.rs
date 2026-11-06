// SPDX-License-Identifier: CC0-1.0

//! The `Model` records API Gateway creates every REST API WITH, before a caller has
//! created any.
//!
//! A REST API is never empty of models: API Gateway seeds each one with two models of its
//! own — `Empty`, the schema of a payload with no declared structure, and `Error`, the
//! schema of the error payload its gateway responses carry. A console- or
//! definition-authored API's method responses name them (`"application/json": "Empty"` is
//! what the console puts on a method response it creates), so an API that does not hold
//! them cannot be configured the way one created through AWS can.
//!
//! The two records are ordinary stored models from the moment the API exists: they are
//! read by `GetModel`, listed by `GetModels`, generated from by `GetModelTemplate`,
//! referenced by a method's `requestModels` and a method response's `responseModels`,
//! patched by `UpdateModel` and removable by `DeleteModel` — nothing here is special-cased
//! on a later read. Seeding is therefore the whole of it: [`seed`] is called by each
//! operation that CREATES a REST API, so one definition of the defaults serves every
//! create path.
//!
//! A child module of [`super`] rather than more of that file: the schema documents are
//! long, and they belong together with the one function that stores them.

use super::{ComAmazonawsApigatewayBackplanecontrolserviceState, ModelEntry, RestApiEntry};

/// The content type the seeded models describe a payload of: both default schemas are
/// JSON Schema documents, so the payload they type is JSON.
pub const DEFAULT_MODEL_CONTENT_TYPE: &str = "application/json";

/// The `Empty` model's name, as `Method$requestModels` and `MethodResponse$responseModels`
/// name it.
pub const EMPTY_MODEL_NAME: &str = "Empty";

/// The `Error` model's name; see [`EMPTY_MODEL_NAME`].
pub const ERROR_MODEL_NAME: &str = "Error";

/// The `Empty` model's description, the sentence API Gateway stores on it.
const EMPTY_MODEL_DESCRIPTION: &str = "This is a default empty schema model";

/// The `Error` model's description; see [`EMPTY_MODEL_DESCRIPTION`].
const ERROR_MODEL_DESCRIPTION: &str = "This is a default error schema model";

/// The `Empty` model's schema: a draft-04 document titled `Empty Schema` that declares an
/// object and no property of it, so it describes any JSON object payload.
///
/// Stored as the exact text API Gateway reports for it — two-space indentation and a space
/// either side of each `:` — because the schema is a STRING member of `Model`, so its
/// formatting is part of what a reader gets back.
const EMPTY_MODEL_SCHEMA: &str = "{\n  \
     \"$schema\" : \"http://json-schema.org/draft-04/schema#\",\n  \
     \"title\" : \"Empty Schema\",\n  \
     \"type\" : \"object\"\n\
     }";

/// The `Error` model's schema: a draft-04 document titled `Error Schema` declaring the one
/// `message` string an API Gateway error payload carries. Formatted as
/// [`EMPTY_MODEL_SCHEMA`] is, and for the same reason.
const ERROR_MODEL_SCHEMA: &str = "{\n  \
     \"$schema\" : \"http://json-schema.org/draft-04/schema#\",\n  \
     \"title\" : \"Error Schema\",\n  \
     \"type\" : \"object\",\n  \
     \"properties\" : {\n    \
     \"message\" : { \"type\" : \"string\" }\n  \
     }\n\
     }";

/// The defaults in the order they are created, as `(name, description, schema)`.
const DEFAULTS: [(&str, &str, &str); 2] = [
    (
        EMPTY_MODEL_NAME,
        EMPTY_MODEL_DESCRIPTION,
        EMPTY_MODEL_SCHEMA,
    ),
    (
        ERROR_MODEL_NAME,
        ERROR_MODEL_DESCRIPTION,
        ERROR_MODEL_SCHEMA,
    ),
];

/// The names of the models every created API holds, in creation order.
#[must_use]
pub fn default_model_names() -> Vec<String> {
    DEFAULTS
        .iter()
        .map(|(name, _, _)| (*name).to_string())
        .collect()
}

/// Store the default models on `api`, minting an id and a creation rank for each out of
/// `state`.
///
/// Called by every operation that CREATES a REST API, while it holds the store and before
/// the API is inserted, so the record that reaches the store already holds them and no
/// read can observe an API without them. Each model is minted exactly as
/// `CreateModel` mints one — an id from the store's own minter and the next creation rank —
/// so the defaults are listed before anything a caller creates and carry ids of the same
/// form as every other model's.
///
/// A name the API already holds a model under is NOT overwritten: an import that declared
/// its own `Empty` has stated what that name means for that API.
pub fn seed(api: &mut RestApiEntry, state: &mut ComAmazonawsApigatewayBackplanecontrolserviceState) {
    for (name, description, schema) in DEFAULTS {
        if api.models.contains_key(name) {
            continue;
        }
        let id = state.mint_id();
        let sequence = state.next_sequence();
        api.models.insert(
            name.to_string(),
            ModelEntry {
                id,
                name: name.to_string(),
                description: Some(description.to_string()),
                content_type: DEFAULT_MODEL_CONTENT_TYPE.to_string(),
                schema: Some(schema.to_string()),
                sequence,
            },
        );
    }
}
