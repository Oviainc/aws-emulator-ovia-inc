// SPDX-License-Identifier: CC0-1.0

//! The ACTION TYPE catalog: the identity of an action type, the records this account
//! holds for the custom ones, the validation a declaration of one has to pass, and the
//! action types the service itself provides.
//!
//! Split out of [`super`] so one file holds one concern (and so the module root stays
//! within the project's file-length limit); every name stays re-exported from
//! `crate::state`, which is where the handlers reach it.

use super::*;

/// An action type's identity: the four `ActionTypeId` members, in the order the
/// shape declares them (`smithy_query query="shape" shape="ActionTypeId"` ->
/// `category`, `owner`, `provider`, `version`).
///
/// Ordered, so a map keyed by it pages deterministically, and the order a listing
/// reports is the same across calls.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ActionTypeKey {
    /// An `ActionCategory` value.
    pub category: String,
    /// An `ActionOwner` value.
    pub owner: String,
    /// The provider of the service the action calls.
    pub provider: String,
    /// The action type's version.
    pub version: String,
}

impl ActionTypeKey {
    /// The identity a request's `ActionTypeId` names.
    pub fn from_id(id: &ActionTypeId) -> Self {
        Self {
            category: id.category.clone(),
            owner: id.owner.clone(),
            provider: id.provider.clone(),
            version: id.version.clone(),
        }
    }

    /// This identity as the modelled `ActionTypeId` a response carries.
    pub fn to_id(&self) -> ActionTypeId {
        ActionTypeId {
            category: self.category.clone(),
            owner: self.owner.clone(),
            provider: self.provider.clone(),
            version: self.version.clone(),
        }
    }

    /// The identity an action-type DEFINITION names.
    ///
    /// `ActionTypeIdentifier` and `ActionTypeId` declare the same four members —
    /// `ActionTypeIdentifier$owner` is the narrower of the two (`@pattern`
    /// `^AWS|ThirdParty$`, so a definition never names a `Custom` action type) — so
    /// one key type addresses an action type however a request names it.
    pub fn from_identifier(id: &ActionTypeIdentifier) -> Self {
        Self {
            category: id.category.clone(),
            owner: id.owner.clone(),
            provider: id.provider.clone(),
            version: id.version.clone(),
        }
    }

    /// This identity as the ordered key a pagination cursor names.
    ///
    /// `/` separates the members because neither `ActionProvider` nor `Version` can
    /// contain one (both are `@pattern("^[0-9A-Za-z_-]+$")`) and `ActionCategory` and
    /// `ActionOwner` are enums of bare words, so the rendering is unambiguous and
    /// [`ActionTypeKey::from_cursor`] recovers exactly the four members.
    pub fn cursor(&self) -> String {
        format!(
            "{}/{}/{}/{}",
            self.category, self.owner, self.provider, self.version
        )
    }

    /// The identity a cursor key names, or `None` when it is not one
    /// [`ActionTypeKey::cursor`] produced.
    pub fn from_cursor(cursor: &str) -> Option<Self> {
        let mut parts = cursor.split('/');
        let key = Self {
            category: parts.next()?.to_string(),
            owner: parts.next()?.to_string(),
            provider: parts.next()?.to_string(),
            version: parts.next()?.to_string(),
        };
        if parts.next().is_some()
            || [&key.category, &key.owner, &key.provider, &key.version]
                .iter()
                .any(|member| member.is_empty())
        {
            return None;
        }
        Some(key)
    }
}

/// One action type as this service holds it: everything `ActionType` reports beside
/// its id (which is the key it is stored under), plus the Region it is available in
/// and the tags a create applied to it.
#[derive(Debug, Clone, Default)]
pub struct ActionTypeRecord {
    /// URLs that tell a user about this action type, when it has any.
    pub settings: Option<ActionTypeSettings>,
    /// The configuration properties the action accepts.
    pub configuration_properties: Vec<ActionConfigurationProperty>,
    /// How many input artifacts the action takes.
    pub input_artifact_details: ArtifactDetails,
    /// How many output artifacts the action produces.
    pub output_artifact_details: ArtifactDetails,
    /// The Region this action type is registered in, or `None` for one the service
    /// offers in every Region. `ListActionTypes$regionFilter` ("The Region to filter
    /// on for the list of action types") is what reads it.
    pub region: Option<String>,
    /// The tags applied to this action type, as `CreateCustomActionType` stored them
    /// and a later `TagResource` call merged into them. Empty for an action type the
    /// service provides itself, which no caller tagged (and which no caller can tag:
    /// it belongs to no account). `ActionType` carries no `tags` member, so only an
    /// operation that reports a resource's tags reads it: `CreateCustomActionType`'s
    /// own answer and `ListTagsForResource`.
    pub tags: Vec<Tag>,
}

impl ActionTypeRecord {
    /// Whether this action type is one a caller in `region` can use: a registration
    /// belongs to the Region it was made in, and an action type the service offers
    /// everywhere belongs to all of them.
    pub fn available_in(&self, region: &str) -> bool {
        self.region.as_deref().is_none_or(|own| own == region)
    }

    /// This action type as `ListActionTypes` and `CreateCustomActionType` report it.
    ///
    /// Every member `ActionType` declares is REPORTED, whether or not this record holds
    /// anything for it: an action type with no URLs carries `settings` as the empty
    /// structure and one with no configuration parameters carries
    /// `actionConfigurationProperties` as the empty list, because a client reads the
    /// properties it must fill in off that list and an absent member is not a list it
    /// can read as "none" — the same reason `ActionType$actionConfigurationProperties`
    /// is modelled `@length(min: 0)` rather than as a member that may be missing.
    pub fn to_action_type(&self, key: &ActionTypeKey) -> ActionType {
        ActionType {
            id: key.to_id(),
            settings: Some(self.settings.clone().unwrap_or_default()),
            action_configuration_properties: Some(self.configuration_properties.clone()),
            input_artifact_details: self.input_artifact_details.clone(),
            output_artifact_details: self.output_artifact_details.clone(),
        }
    }
}

/// Refuse an `ActionTypeDeclaration` the service's own documented rules rule out,
/// reporting why. `Ok(())` means the definition is storable.
///
/// None of these is a Smithy constraint (every one of those is refused before a
/// handler runs), and they are the same rules
/// [`validate_configuration_properties`] applies to the other view of an action
/// type's properties:
///
/// * STATED by `ActionTypeProperty$queryable`: "An action type can have up to one
///   queryable property. If it has one, that property must be both required and not
///   secret" — a property is required when `optional` is false and secret when
///   `noEcho` is true.
/// * INFERRED from `ActionTypeProperty$name` ("The property name that is displayed
///   to users"): a name identifies a property, so a list naming one twice does not
///   describe a usable action type. The model does not say this in so many words.
/// * STATED by `ActionTypeExecutor$type` ("The integration model used to create and
///   update the action type, `Lambda` or `JobWorker`") together with the two members
///   of `ExecutorConfiguration`, each documented as the details of ONE of those
///   models: a definition carries the configuration of the model it declares, and
///   not the other model's.
pub fn validate_action_type_declaration(declaration: &ActionTypeDeclaration) -> Result<(), String> {
    validate_action_type_properties(declaration.properties.as_deref().unwrap_or_default())?;
    let configuration = &declaration.executor.configuration;
    let (declared, other) = match declaration.executor.r#type.as_str() {
        EXECUTOR_TYPE_LAMBDA => (
            configuration.lambda_executor_configuration.is_some(),
            configuration.job_worker_executor_configuration.is_some(),
        ),
        EXECUTOR_TYPE_JOB_WORKER => (
            configuration.job_worker_executor_configuration.is_some(),
            configuration.lambda_executor_configuration.is_some(),
        ),
        // `ExecutorType` is a modelled enum, so the boundary validator has already
        // refused anything that is not one of its two values; a value this build does
        // not know carries no configuration rule.
        _ => (true, false),
    };
    if !declared {
        return Err(format!(
            "executor.configuration carries no details for the {} integration model",
            declaration.executor.r#type
        ));
    }
    if other {
        return Err(format!(
            "executor.configuration carries details for an integration model other than {}",
            declaration.executor.r#type
        ));
    }
    Ok(())
}

/// The property half of [`validate_action_type_declaration`], over the
/// `ActionTypeProperty` view of an action type's configuration properties. Only that
/// check asks, so this one stays private to the module.
fn validate_action_type_properties(properties: &[ActionTypeProperty]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for property in properties {
        if !seen.insert(property.name.as_str()) {
            return Err(format!(
                "properties names the property {} more than once",
                property.name
            ));
        }
    }
    let queryable: Vec<&ActionTypeProperty> = properties
        .iter()
        .filter(|property| property.queryable)
        .collect();
    if queryable.len() > 1 {
        return Err(format!(
            "properties may declare at most one queryable property, but {} are queryable",
            queryable.len()
        ));
    }
    if let Some(property) = queryable.first() {
        if property.optional || property.no_echo {
            return Err(format!(
                "the queryable property {} must be required and not secret",
                property.name
            ));
        }
    }
    Ok(())
}

// The catalog of action types the service provides ITSELF — the entries and their
// per-provider detail — lives in [`super::built_in_catalog`], beside this module rather
// than in it, so neither file holds two concerns.
