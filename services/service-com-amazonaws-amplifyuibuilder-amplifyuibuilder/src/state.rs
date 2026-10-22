// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-amplifyuibuilder-amplifyuibuilder service.
//!
//! The component is the primary resource of the Amplify UI Builder control
//! plane: `CreateComponent` mints one, `GetComponent` / `ListComponents` report
//! it, `UpdateComponent` merges into it and `DeleteComponent` removes it. The
//! record below is the one copy of a component every one of those handlers
//! reads and writes, plus the two projections the model asks for (`Component`
//! and `ComponentSummary`).
//!
//! Beside the components, the store holds one app environment's UI-builder
//! metadata: the feature flags `PutMetadataFlag` writes and `GetMetadata`
//! reports.
//!
//! ## The codegen job, and the clock that moves its status
//!
//! `StartCodegenJob` mints the service's other resource, the CODEGEN JOB, and
//! `GetCodegenJob` reports it. The job AWS returns is `in_progress` — a
//! non-terminal status — so the service owes it a terminal status without
//! anybody calling again. That owed transition is held as a DUE INSTANT
//! ([`StoredCodegenJob::next_status_due`]) and materialised by
//! [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::settle_codegen_jobs`],
//! which each codegen handler calls first: a read reports what the clock has
//! already made due and never advances a job because it was polled.
//!
//! The form and the theme are the control plane's other app-environment
//! resources, held the same way: `CreateForm` mints a form and `GetForm` reports
//! it from the single [`StoredForm`] record, `CreateTheme` mints a theme and
//! `GetTheme` reports it from the single [`StoredTheme`] record, both keyed by
//! the same (appId, environmentName) a component is keyed by and both with the
//! projection the model asks for.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CodegenFeatureFlags, CodegenJob, CodegenJobGenericDataSchema, CodegenJobRenderConfig,
    Component, ComponentBindingPropertiesValue, ComponentChild, ComponentDataConfiguration,
    ComponentEvent, ComponentProperty, ComponentSummary, ComponentVariant, FieldConfig, Form,
    FormCTA, FormDataTypeConfig, FormStyle, InvalidParameterException, ResourceConflictException,
    ResourceNotFoundException, SectionalElement, Theme, ThemeValues,
};

/// The largest page `ListComponents` serves, and the page size it uses when the
/// request names no `maxResults`: that member targets `ListEntityLimit`, which
/// the model constrains `@range(min: 1, max: 100)`.
pub const COMPONENT_PAGE_LIMIT: i32 = 100;

/// How many components one `ExportComponents` page carries.
///
/// `ExportComponentsRequest` models no page-size member (only `nextToken`), so
/// the length of an export page is the service's own choice rather than
/// something a caller asks for. This emulator serves the same largest page its
/// component list does, so a caller walking either operation walks pages of one
/// bound.
pub const EXPORT_PAGE_LIMIT: usize = COMPONENT_PAGE_LIMIT as usize;

/// How many components the store holds before `CreateComponent` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no component quota this emulator can quote, so
/// this is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the state growing without end: every record it holds is
/// released by `DeleteComponent`, which also forgets that component's client
/// tokens, so the token map can never outgrow this bound either.
pub const COMPONENT_QUOTA: usize = 1_000;

/// The feature names this service's metadata holds — the catalogue
/// `PutMetadataFlag` accepts and `GetMetadata` reports.
///
/// `FeaturesMap` is modelled as an open `String -> String` map and names no
/// feature (`smithy_query(query="shape",shape="FeaturesMap")`), so the catalogue
/// cannot be read off the model: it is the fixed set of UI-builder feature flags
/// `GetMetadata` reports for an app environment, which is what `PutMetadataFlag`
/// validates `featureName` against — any other name is refused rather than
/// stored.
pub const METADATA_FEATURES: [&str; 5] = [
    "autoGenerateForms",
    "autoGenerateViews",
    "isRelationshipSupported",
    "isNonModelSupported",
    "isGraphQLEnabled",
];

/// What `GetMetadata` reports for a feature no `PutMetadataFlag` has set.
///
/// Every feature in [`METADATA_FEATURES`] is a flag, and `PutMetadataFlagBody$newValue`
/// is the free-form string a caller sets it to, so an app environment nothing has
/// written reports every one of them off. This is the emulator's OWN starting
/// value — AWS publishes no default for these flags — and it moves only when a
/// `PutMetadataFlag` writes one.
pub const METADATA_FEATURE_DEFAULT: &str = "false";

/// How many app environments the metadata store holds flags for before
/// `PutMetadataFlag` refuses to start another one.
///
/// Amplify UI Builder publishes no quota on app environments this emulator can
/// quote, so — exactly like [`COMPONENT_QUOTA`] — this is the emulator's OWN
/// bound on the store rather than a claim about any account's limit, and it is
/// what stops the state growing without end: `appId` and `environmentName` are
/// request-supplied `@httpLabel` values, so without a bound every request naming
/// a fresh pair would add an entry nothing ever releases (the service models no
/// delete-metadata operation). It is set far above any workflow an emulator
/// serves. Each entry holds at most [`METADATA_FEATURES`]`.len()` values —
/// `PutMetadataFlag` refuses every other feature name — so the whole store is
/// bounded by this many entries times that many values, each value no longer
/// than the request that wrote it.
pub const METADATA_ENVIRONMENT_QUOTA: usize = 1_000;

/// How many forms the store holds before `CreateForm` refuses with the modelled
/// `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no form quota this emulator can quote, so — like
/// [`COMPONENT_QUOTA`] — this is the emulator's OWN bound on the store rather
/// than a claim about any account's limit, set far above any workflow an
/// emulator serves.
pub const FORM_QUOTA: usize = 1_000;

/// How many themes the store holds before `CreateTheme` refuses with the
/// modelled `ServiceQuotaExceededException`.
///
/// Amplify UI Builder publishes no theme quota this emulator can quote, so this
/// is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It is set far above any workflow an emulator serves, and it
/// is what stops the theme state — the records and the client tokens that name
/// them — growing without end.
pub const THEME_QUOTA: usize = 1_000;

/// The map key a component's property maps may not carry.
///
/// `Component$properties`, `$overrides`, `$bindingProperties` and
/// `$collectionProperties` each document "You can't specify `tags` as a valid
/// property" for that map, so a request naming it is refused rather than stored.
pub const RESERVED_PROPERTY_NAME: &str = "tags";

/// The `CodegenJobStatus` values the model declares, so no handler spells one as
/// a bare literal. The full set is `ops::types::CODEGEN_JOB_STATUS_VALUES`.
pub const CODEGEN_JOB_IN_PROGRESS: &str = "in_progress";
/// See [`CODEGEN_JOB_IN_PROGRESS`].
pub const CODEGEN_JOB_FAILED: &str = "failed";
/// See [`CODEGEN_JOB_IN_PROGRESS`].
pub const CODEGEN_JOB_SUCCEEDED: &str = "succeeded";

/// The statuses a codegen job never leaves. `in_progress` is the only transient
/// one, so a job in either of these owes the clock nothing.
pub const CODEGEN_JOB_TERMINAL_STATUSES: &[&str] = &[CODEGEN_JOB_FAILED, CODEGEN_JOB_SUCCEEDED];

/// How long after `StartCodegenJob` the clock owes the job its terminal status.
///
/// AWS documents no duration — how long a code generation run takes depends on
/// the app being rendered — so this span is CHOSEN, not modelled: 500 ms, inside
/// the project's 250-750 ms band. It is deliberately non-zero, so a caller
/// reading the job straight after starting it still observes `in_progress`.
/// Nothing sleeps for it: the due instant is stored and compared to the clock.
pub const CODEGEN_JOB_TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// Why a codegen job this emulator accepts ends `failed`.
///
/// `StartCodegenJob` starts a RUN of the Amplify UI code generator, which renders
/// the app's components, forms and themes into React sources and publishes them
/// as the artifact `CodegenJob$asset.downloadUrl` points at. This emulator has no
/// such generator and no artifact store to serve a download from, so the run is
/// not performed: the job reaches the service's modelled failure status carrying
/// this reason, rather than reporting a success nothing computed.
pub const CODEGEN_JOB_NOT_RUN: &str = "this emulator does not run the Amplify UI code generator, \
     so no code was generated and no asset was published for this job";

/// How many codegen jobs the store holds before `StartCodegenJob` refuses.
///
/// A started job is reported for the life of the process — `GetCodegenJob` must
/// still answer what `StartCodegenJob` created, so nothing evicts one — and
/// starting a job is the only thing that adds one, so the store needs a maximum
/// entry count. Amplify UI Builder publishes no codegen-job quota this emulator
/// can quote, so this is the emulator's OWN bound, set far above any workflow an
/// emulator serves.
pub const CODEGEN_JOB_QUOTA: usize = 1_000;

/// How a `ListComponents` page token is spelled, so a token this operation never
/// issued can be told apart from one it did.
const PAGE_TOKEN_PREFIX: &str = "c-";

/// The width of the cursor a page token carries, in hex digits (a `u64`).
const PAGE_TOKEN_HEX_WIDTH: usize = 16;

/// The width of the app-environment digest a page token carries, in hex digits
/// (a `u32`).
const PAGE_TOKEN_SCOPE_WIDTH: usize = 8;

/// Which Amplify app and backend environment a component belongs to: the two
/// `@httpLabel` members that, with the id, name a component.
pub type EnvironmentKey = (String, String);

/// One Amplify UI Builder component, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredComponent {
    pub app_id: String,
    pub environment_name: String,
    /// `Component$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_component_id`].
    pub id: String,
    pub name: String,
    pub component_type: String,
    pub properties: HashMap<String, ComponentProperty>,
    pub children: Option<Vec<ComponentChild>>,
    pub variants: Vec<ComponentVariant>,
    pub overrides: HashMap<String, HashMap<String, String>>,
    pub binding_properties: HashMap<String, ComponentBindingPropertiesValue>,
    pub collection_properties: Option<HashMap<String, ComponentDataConfiguration>>,
    pub events: Option<HashMap<String, ComponentEvent>>,
    pub source_id: Option<String>,
    pub schema_version: Option<String>,
    /// `Component$tags` is reported for every component, so the tags a create
    /// supplied are held as a map rather than as an absence: a component created
    /// with no tags carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the component last changed. Creating it is the change that first set
    /// it, so it starts at [`StoredComponent::created_at`] and is moved by every
    /// `UpdateComponent` that merges into the record.
    pub modified_at: SystemTime,
    /// The position this component holds in its environment's creation order.
    ///
    /// It is what a `ListComponents` page token indexes into: it never changes
    /// and never repeats, so a page resumes at the same place whether or not
    /// components were created or deleted since the previous page.
    pub cursor: u64,
}

impl StoredComponent {
    /// The `Component` the create, get and update operations return.
    pub fn to_component(&self) -> Component {
        Component {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            source_id: self.source_id.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
            properties: self.properties.clone(),
            children: self.children.clone(),
            variants: self.variants.clone(),
            overrides: self.overrides.clone(),
            binding_properties: self.binding_properties.clone(),
            collection_properties: self.collection_properties.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            tags: Some(self.tags.clone()),
            events: self.events.clone(),
            schema_version: self.schema_version.clone(),
        }
    }

    /// The `ComponentSummary` projection `ListComponents` returns.
    pub fn to_summary(&self) -> ComponentSummary {
        ComponentSummary {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            component_type: self.component_type.clone(),
        }
    }
}

/// One Amplify UI Builder code generation job, as this emulator persists it.
///
/// Every member a modelled response carries is read from here; nothing in a
/// response is recomputed from the request.
#[derive(Debug, Clone)]
pub struct StoredCodegenJob {
    pub app_id: String,
    pub environment_name: String,
    /// `CodegenJob$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_codegen_job_id`].
    pub id: String,
    /// The job configuration `StartCodegenJobData` supplied, held as given:
    /// `CodegenJob` reports each of these back as `create-and-read` members.
    pub render_config: CodegenJobRenderConfig,
    pub generic_data_schema: Option<CodegenJobGenericDataSchema>,
    pub auto_generate_forms: Option<bool>,
    pub features: Option<CodegenFeatureFlags>,
    /// `CodegenJob$tags` is reported for every job, so the tags a start supplied
    /// are held as a map rather than as an absence: a job started with no tags
    /// carries an empty one and reports `{}`.
    pub tags: HashMap<String, String>,
    /// One of [`CODEGEN_JOB_IN_PROGRESS`] or [`CODEGEN_JOB_TERMINAL_STATUSES`].
    pub status: String,
    pub status_message: Option<String>,
    pub created_at: SystemTime,
    /// When the job last changed. Starting it is the change that first set it, so
    /// it begins at [`StoredCodegenJob::created_at`] and moves to the instant the
    /// status transition below was due.
    pub modified_at: SystemTime,
    /// The status the clock owes this job, and the instant it is owed from.
    /// `None` for a job in a terminal status, which owes nothing further.
    pub next_status_due: Option<(SystemTime, &'static str)>,
}

impl StoredCodegenJob {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        CODEGEN_JOB_TERMINAL_STATUSES.contains(&self.status.as_str())
    }

    /// Apply the owed status transition when the clock has already made it due.
    ///
    /// Only a change that was owed at `now` whether or not anybody called is
    /// applied here, so calling this from a reporting operation does not move a
    /// job the clock has not moved yet.
    fn settle(&mut self, now: SystemTime) {
        let Some((due, next)) = self.next_status_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        // The reason is DERIVED from the status reached, so the two can never
        // disagree: `failed` says why the run did not happen, and a job that
        // reached any other status carries no failure reason.
        self.status_message = (next == CODEGEN_JOB_FAILED).then(|| CODEGEN_JOB_NOT_RUN.to_string());
        self.next_status_due = None;
        // The transition is the change, and it was due at `due` — not at the
        // instant somebody happened to read the job.
        self.modified_at = due;
        debug_assert!(
            self.is_terminal(),
            "a settled codegen job holds a terminal status, not {:?}",
            self.status
        );
    }

    /// The `CodegenJob` `StartCodegenJob` and `GetCodegenJob` return.
    pub fn to_codegen_job(&self) -> CodegenJob {
        CodegenJob {
            id: self.id.clone(),
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            render_config: Some(self.render_config.clone()),
            generic_data_schema: self.generic_data_schema.clone(),
            auto_generate_forms: self.auto_generate_forms,
            features: self.features.clone(),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            // `asset` carries the download URL of the artifact a code generation
            // RUN publishes, and `dependencies` the packages that run found the
            // generated code needs. This emulator performs no such run (see
            // [`CODEGEN_JOB_NOT_RUN`]), so both members are ABSENT rather than
            // carrying a URL nothing serves or a dependency list nothing computed.
            asset: None,
            tags: Some(self.tags.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            dependencies: None,
        }
    }
}

/// One Amplify UI Builder form, as this emulator persists it.
///
/// `Form` models no timestamps (`smithy_query(query="shape", shape="Form")`
/// lists appId, environmentName, id, name, formActionType, style, dataType,
/// fields, sectionalElements, schemaVersion, tags, cta and labelDecorator), so
/// none is held: every field below is one the model reports.
#[derive(Debug, Clone)]
pub struct StoredForm {
    pub app_id: String,
    pub environment_name: String,
    /// `Form$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_form_id`].
    pub id: String,
    pub name: String,
    pub form_action_type: String,
    pub style: FormStyle,
    pub data_type: FormDataTypeConfig,
    pub fields: HashMap<String, FieldConfig>,
    pub sectional_elements: HashMap<String, SectionalElement>,
    pub schema_version: String,
    /// `Form$tags` is optional on both `CreateFormData` and `Form`, so the
    /// absence of tags is held as an absence rather than as an empty map: a form
    /// created without them reports no `tags` member at all.
    pub tags: Option<HashMap<String, String>>,
    pub cta: Option<FormCTA>,
    pub label_decorator: Option<String>,
}

impl StoredForm {
    /// The `Form` the create and get operations return.
    pub fn to_form(&self) -> Form {
        Form {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            form_action_type: self.form_action_type.clone(),
            style: self.style.clone(),
            data_type: self.data_type.clone(),
            fields: self.fields.clone(),
            sectional_elements: self.sectional_elements.clone(),
            schema_version: self.schema_version.clone(),
            tags: self.tags.clone(),
            cta: self.cta.clone(),
            label_decorator: self.label_decorator.clone(),
        }
    }
}

/// One Amplify UI Builder theme, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredTheme {
    pub app_id: String,
    pub environment_name: String,
    /// `Theme$id` targets `Uuid`; minted by
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::mint_theme_id`].
    pub id: String,
    pub name: String,
    /// `Theme$values` is `@required`: the list the create supplied, in the order
    /// it supplied it, nested `ThemeValue$children` and all.
    pub values: Vec<ThemeValues>,
    /// `Theme$overrides` is optional, so an absent one stays absent rather than
    /// being reported as an empty list.
    pub overrides: Option<Vec<ThemeValues>>,
    /// `Theme$tags` is reported for every theme, so the tags a create supplied
    /// are held as a map rather than as an absence: a theme created with no tags
    /// carries an empty one and reports `{}`, exactly as a component does.
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    /// When the theme last changed. Creating it is the change that first set it,
    /// so it starts at [`StoredTheme::created_at`].
    pub modified_at: SystemTime,
}

impl StoredTheme {
    /// The `Theme` the create and get operations return.
    pub fn to_theme(&self) -> Theme {
        Theme {
            app_id: self.app_id.clone(),
            environment_name: self.environment_name.clone(),
            id: self.id.clone(),
            name: self.name.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            modified_at: Some(::aws_smithy_types::DateTime::from(self.modified_at)),
            values: self.values.clone(),
            overrides: self.overrides.clone(),
            tags: Some(self.tags.clone()),
        }
    }
}

/// Per-service state for com-amazonaws-amplifyuibuilder-amplifyuibuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// Every component, keyed by the (appId, environmentName) it belongs to and
    /// held in creation order within that environment — the stable total order
    /// `ListComponents` pages over.
    pub components: HashMap<EnvironmentKey, Vec<StoredComponent>>,
    /// A `CreateComponent` `clientToken` to the component that token created,
    /// so a retried create reports the first component instead of minting a
    /// second. The key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in, so
    /// reusing one against another app environment creates that environment's
    /// own component rather than reporting a component the request does not
    /// address.
    pub component_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The UI-builder metadata flags written for one app environment, keyed by
    /// the (appId, environmentName) the `PutMetadataFlag` addressed: the feature
    /// name to the value it was last set to. A feature nothing has written is
    /// absent here and reported as [`METADATA_FEATURE_DEFAULT`], so the map holds
    /// only what a caller actually stored, and how many app environments it holds
    /// at all is bounded by [`METADATA_ENVIRONMENT_QUOTA`].
    pub metadata_flags: HashMap<EnvironmentKey, HashMap<String, String>>,
    /// Every codegen job, keyed by the (appId, environmentName) it belongs to and
    /// held in start order within that environment.
    pub codegen_jobs: HashMap<EnvironmentKey, Vec<StoredCodegenJob>>,
    /// A `StartCodegenJob` `clientToken` to the job that token started, so a
    /// retried start reports the first job instead of starting a second. Scoped
    /// to the (appId, environmentName) the start addressed, exactly as
    /// [`ComAmazonawsAmplifyuibuilderAmplifyuibuilderState::component_tokens`] is.
    pub codegen_job_tokens: HashMap<(EnvironmentKey, String), String>,
    /// Every form, keyed by the (appId, environmentName) it belongs to and held
    /// in creation order within that environment.
    pub forms: HashMap<EnvironmentKey, Vec<StoredForm>>,
    /// A `CreateForm` `clientToken` to the form that token created, so a retried
    /// create reports the first form instead of minting a second. As for a
    /// component, the key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in.
    pub form_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The counter [`StoredComponent::cursor`] values are drawn from.
    pub cursor_seq: u64,
    /// Every theme, keyed by the (appId, environmentName) it belongs to and held
    /// in creation order within that environment.
    pub themes: HashMap<EnvironmentKey, Vec<StoredTheme>>,
    /// A `CreateTheme` `clientToken` to the theme that token created, so a
    /// retried create reports the first theme instead of minting a second. As
    /// for a component, the key carries the (appId, environmentName) the create
    /// addressed: a token is scoped to the environment it was used in.
    pub theme_tokens: HashMap<(EnvironmentKey, String), String>,
    /// The seed this store's resource ids are minted from, drawn from the
    /// operating system the first time one is minted.
    id_seed: Option<u64>,
    /// The counter that keeps two ids of one run apart.
    id_seq: u64,
}

impl ComAmazonawsAmplifyuibuilderAmplifyuibuilderState {
    /// The components stored for one app environment, in creation order.
    pub fn components_in(&self, app_id: &str, environment_name: &str) -> &[StoredComponent] {
        self.components
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The component `id` names in one app environment.
    pub fn component(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&StoredComponent> {
        self.components_in(app_id, environment_name)
            .iter()
            .find(|component| component.id == id)
    }

    /// Whether `id` names a component some OTHER app environment owns.
    ///
    /// A component is addressed by (appId, environmentName, id), so an id that
    /// resolves outside the app environment the request named is a request made
    /// in the wrong scope — told apart here from an id that names no component at
    /// all, which is simply absent.
    pub fn component_in_other_scope(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> bool {
        self.components
            .iter()
            .filter(|((app, environment), _)| app != app_id || environment != environment_name)
            .any(|(_, components)| components.iter().any(|component| component.id == id))
    }

    /// The live component called `name` in one app environment, if there is one.
    ///
    /// A component name identifies a component within its app environment, so
    /// this is what a create (or a rename) consults before taking a name.
    pub fn component_named(
        &self,
        app_id: &str,
        environment_name: &str,
        name: &str,
    ) -> Option<&StoredComponent> {
        self.components_in(app_id, environment_name)
            .iter()
            .find(|component| component.name == name)
    }

    /// The component `id` names in one app environment, for a caller that
    /// changes it.
    pub fn component_mut(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&mut StoredComponent> {
        self.components
            .get_mut(&(app_id.to_string(), environment_name.to_string()))?
            .iter_mut()
            .find(|component| component.id == id)
    }

    /// Store a component at the end of its environment's creation order.
    pub fn insert_component(&mut self, component: StoredComponent) {
        self.components
            .entry((
                component.app_id.clone(),
                component.environment_name.clone(),
            ))
            .or_default()
            .push(component);
    }

    /// Remove the component `id` names, and forget every client token that named
    /// it so the token map cannot outlive the record.
    pub fn remove_component(
        &mut self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<StoredComponent> {
        let key = (app_id.to_string(), environment_name.to_string());
        let components = self.components.get_mut(&key)?;
        let index = components
            .iter()
            .position(|component| component.id == id)?;
        let removed = components.remove(index);
        if components.is_empty() {
            self.components.remove(&key);
        }
        self.component_tokens
            .retain(|(token_key, _), token_id| !(*token_key == key && token_id == id));
        Some(removed)
    }

    /// The component one `clientToken` already created in the app environment
    /// the create addressed, if that token has been seen there.
    pub fn replayed_component(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredComponent> {
        let id = self
            .component_tokens
            .get(&(key.clone(), token.to_string()))?;
        self.component(&key.0, &key.1, id)
    }

    /// Remember that `token` created the component `id` in that app environment.
    pub fn remember_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.component_tokens.insert((key, token), id);
    }

    /// The metadata features of one app environment: every feature in
    /// [`METADATA_FEATURES`] with the value a `PutMetadataFlag` last wrote for it,
    /// or [`METADATA_FEATURE_DEFAULT`] for one nothing has written.
    pub fn metadata_features(
        &self,
        app_id: &str,
        environment_name: &str,
    ) -> HashMap<String, String> {
        let written = self
            .metadata_flags
            .get(&(app_id.to_string(), environment_name.to_string()));
        METADATA_FEATURES
            .iter()
            .map(|feature| {
                let value = written
                    .and_then(|flags| flags.get(*feature))
                    .cloned()
                    .unwrap_or_else(|| METADATA_FEATURE_DEFAULT.to_string());
                ((*feature).to_string(), value)
            })
            .collect()
    }

    /// Store the value of one metadata feature of an app environment.
    ///
    /// Writing a feature of an app environment the store does not hold yet starts
    /// an entry, which is what [`METADATA_ENVIRONMENT_QUOTA`] bounds: a caller
    /// checks [`Self::can_hold_metadata_for`] first and refuses past the bound
    /// rather than growing the store without end.
    pub fn set_metadata_flag(&mut self, key: EnvironmentKey, feature: String, value: String) {
        self.metadata_flags
            .entry(key)
            .or_default()
            .insert(feature, value);
    }

    /// How many app environments the metadata store holds flags for — what
    /// [`METADATA_ENVIRONMENT_QUOTA`] bounds.
    pub fn metadata_environments(&self) -> usize {
        self.metadata_flags.len()
    }

    /// Whether a metadata write to `key` is within the store's bound: an app
    /// environment the store already holds always is (the write replaces a value
    /// in an existing entry, and an entry holds at most the catalogue), and a new
    /// one only while the store is below [`METADATA_ENVIRONMENT_QUOTA`].
    pub fn can_hold_metadata_for(&self, key: &EnvironmentKey) -> bool {
        self.metadata_flags.contains_key(key)
            || self.metadata_environments() < METADATA_ENVIRONMENT_QUOTA
    }

    /// How many components the whole store holds — what [`COMPONENT_QUOTA`]
    /// bounds.
    pub fn total_components(&self) -> usize {
        self.components.values().map(Vec::len).sum()
    }

    /// The next cursor in this service's one creation order.
    ///
    /// Saturating rather than wrapping so the "never repeats" invariant above
    /// holds for every value it can actually reach: exhausting a `u64` would
    /// take 2^64 creates, which no emulator run performs.
    pub fn next_cursor(&mut self) -> u64 {
        self.cursor_seq = self.cursor_seq.saturating_add(1);
        self.cursor_seq
    }

    /// The forms stored for one app environment, in creation order.
    pub fn forms_in(&self, app_id: &str, environment_name: &str) -> &[StoredForm] {
        self.forms
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The form `id` names in one app environment.
    pub fn form(&self, app_id: &str, environment_name: &str, id: &str) -> Option<&StoredForm> {
        self.forms_in(app_id, environment_name)
            .iter()
            .find(|form| form.id == id)
    }

    /// Store a form at the end of its environment's creation order.
    pub fn insert_form(&mut self, form: StoredForm) {
        self.forms
            .entry((form.app_id.clone(), form.environment_name.clone()))
            .or_default()
            .push(form);
    }

    /// The form one `clientToken` already created in the app environment the
    /// create addressed, if that token has been seen there.
    pub fn replayed_form(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredForm> {
        let id = self.form_tokens.get(&(key.clone(), token.to_string()))?;
        self.form(&key.0, &key.1, id)
    }

    /// Remember that `token` created the form `id` in that app environment.
    pub fn remember_form_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.form_tokens.insert((key, token), id);
    }

    /// How many forms the whole store holds — what [`FORM_QUOTA`] bounds.
    pub fn total_forms(&self) -> usize {
        self.forms.values().map(Vec::len).sum()
    }

    /// Mint an unused component id. `Component$id` targets `Uuid`, so the id is
    /// laid out as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_component_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .components
                .get(key)
                .is_none_or(|components| components.iter().all(|c| c.id != id))
            {
                return id;
            }
        }
    }

    /// Materialise every codegen-job status transition the clock has already made
    /// due. Call this FIRST in every codegen handler, including a read: it only
    /// applies a change that was owed at `now` whether or not anybody called.
    pub fn settle_codegen_jobs(&mut self, now: SystemTime) {
        for jobs in self.codegen_jobs.values_mut() {
            for job in jobs.iter_mut() {
                job.settle(now);
            }
        }
    }

    /// The codegen jobs stored for one app environment, in start order.
    pub fn codegen_jobs_in(&self, app_id: &str, environment_name: &str) -> &[StoredCodegenJob] {
        self.codegen_jobs
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The codegen job `id` names in one app environment.
    pub fn codegen_job(
        &self,
        app_id: &str,
        environment_name: &str,
        id: &str,
    ) -> Option<&StoredCodegenJob> {
        self.codegen_jobs_in(app_id, environment_name)
            .iter()
            .find(|job| job.id == id)
    }

    /// Store a codegen job at the end of its environment's start order.
    pub fn insert_codegen_job(&mut self, job: StoredCodegenJob) {
        self.codegen_jobs
            .entry((job.app_id.clone(), job.environment_name.clone()))
            .or_default()
            .push(job);
    }

    /// The codegen job one `clientToken` already started in the app environment
    /// the start addressed, if that token has been seen there.
    pub fn replayed_codegen_job(
        &self,
        key: &EnvironmentKey,
        token: &str,
    ) -> Option<&StoredCodegenJob> {
        let id = self
            .codegen_job_tokens
            .get(&(key.clone(), token.to_string()))?;
        self.codegen_job(&key.0, &key.1, id)
    }

    /// Remember that `token` started the codegen job `id` in that app environment.
    pub fn remember_codegen_job_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.codegen_job_tokens.insert((key, token), id);
    }

    /// How many codegen jobs the whole store holds — what [`CODEGEN_JOB_QUOTA`]
    /// bounds.
    pub fn total_codegen_jobs(&self) -> usize {
        self.codegen_jobs.values().map(Vec::len).sum()
    }

    /// Mint an unused codegen job id. `CodegenJob$id` targets `Uuid`, so the id is
    /// laid out as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_codegen_job_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .codegen_jobs
                .get(key)
                .is_none_or(|jobs| jobs.iter().all(|job| job.id != id))
            {
                return id;
            }
        }
    }

    /// Mint an unused form id. `Form$id` targets `Uuid`, so the id is laid out
    /// as a version 4 UUID; the loop skips an id already taken in that
    /// environment.
    pub fn mint_form_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .forms
                .get(key)
                .is_none_or(|forms| forms.iter().all(|form| form.id != id))
            {
                return id;
            }
        }
    }

    /// The themes stored for one app environment, in creation order.
    pub fn themes_in(&self, app_id: &str, environment_name: &str) -> &[StoredTheme] {
        self.themes
            .get(&(app_id.to_string(), environment_name.to_string()))
            .map_or(&[], Vec::as_slice)
    }

    /// The theme `id` names in one app environment.
    pub fn theme(&self, app_id: &str, environment_name: &str, id: &str) -> Option<&StoredTheme> {
        self.themes_in(app_id, environment_name)
            .iter()
            .find(|theme| theme.id == id)
    }

    /// Store a theme at the end of its environment's creation order.
    pub fn insert_theme(&mut self, theme: StoredTheme) {
        self.themes
            .entry((theme.app_id.clone(), theme.environment_name.clone()))
            .or_default()
            .push(theme);
    }

    /// The theme one `clientToken` already created in the app environment the
    /// create addressed, if that token has been seen there.
    pub fn replayed_theme(&self, key: &EnvironmentKey, token: &str) -> Option<&StoredTheme> {
        let id = self.theme_tokens.get(&(key.clone(), token.to_string()))?;
        self.theme(&key.0, &key.1, id)
    }

    /// Remember that `token` created the theme `id` in that app environment.
    pub fn remember_theme_token(&mut self, key: EnvironmentKey, token: String, id: String) {
        self.theme_tokens.insert((key, token), id);
    }

    /// How many themes the whole store holds — what [`THEME_QUOTA`] bounds.
    pub fn total_themes(&self) -> usize {
        self.themes.values().map(Vec::len).sum()
    }

    /// Mint an unused theme id. `Theme$id` targets `Uuid`, like a component's,
    /// so the id is laid out the same way; the loop skips an id already taken in
    /// that environment.
    pub fn mint_theme_id(&mut self, key: &EnvironmentKey) -> String {
        loop {
            let id = self.next_uuid();
            if self
                .themes
                .get(key)
                .is_none_or(|themes| themes.iter().all(|theme| theme.id != id))
            {
                return id;
            }
        }
    }

    /// One candidate id: a version 4 UUID whose 122 free bits are the leading
    /// bits of a SHA-256 digest over this store's seed, a counter and the clock.
    ///
    /// Shared by every `Uuid` this service mints — a component id and a codegen
    /// job id are drawn from the same counter, so two resources of one run never
    /// collide. The seed is drawn from the operating system once per store, so ids
    /// are not predictable from one run to the next; the counter and the clock
    /// make two ids of one run differ even on a seed collision. `sha2` does the
    /// hashing and `hex` the rendering — this only composes the input and sets
    /// the two nibbles RFC 4122 reserves. Every resource this service mints an
    /// id for draws from this one counter, so two resources cannot be handed the
    /// same id either.
    fn next_uuid(&mut self) -> String {
        use sha2::{Digest, Sha256};

        let seed = *self.id_seed.get_or_insert_with(os_seed);
        self.id_seq = self.id_seq.saturating_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());

        let mut digest = Sha256::new();
        digest.update(seed.to_le_bytes());
        digest.update(self.id_seq.to_le_bytes());
        digest.update(nanos.to_le_bytes());
        let digest = digest.finalize();

        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        // RFC 4122 §4.4: the high nibble of octet 6 is the version (4) and the
        // two high bits of octet 8 are the variant (10).
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;

        let hex = hex::encode(bytes);
        format!(
            "{}-{}-{}-{}-{}",
            &hex[..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..]
        )
    }
}

/// A seed from the operating system, for the ids one store mints.
///
/// `RandomState` draws its hasher keys from the OS when it is constructed, so a
/// fresh instance's hash of the empty input is OS-drawn entropy — the standard
/// library's own source, with no further dependency.
fn os_seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};

    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

/// True when a component property map carries the reserved
/// [`RESERVED_PROPERTY_NAME`] key.
///
/// Only the four maps the model documents the rule for are checked by the
/// handlers; `ComponentVariant$overrides` and `ComponentChild$properties` carry
/// no such statement, so a key named `tags` there is stored as given rather than
/// refused on a rule the model does not make.
pub fn carries_reserved_name<V>(map: &HashMap<String, V>) -> bool {
    map.contains_key(RESERVED_PROPERTY_NAME)
}

/// The `nextToken` a page of `key`'s components that ends before `cursor`
/// reports.
///
/// The token carries the app environment it was issued for as well as the
/// cursor, so a token cannot be replayed against another (appId,
/// environmentName): the pages a caller walks are the ones of the list it
/// started.
pub fn page_token(key: &EnvironmentKey, cursor: u64) -> String {
    format!(
        "{PAGE_TOKEN_PREFIX}{scope:0scope_width$x}{cursor:0cursor_width$x}",
        scope = token_scope(key),
        scope_width = PAGE_TOKEN_SCOPE_WIDTH,
        cursor_width = PAGE_TOKEN_HEX_WIDTH
    )
}

/// The cursor `token` resumes `key`'s list at, or `None` when it is not a token
/// `ListComponents` issued for that app environment.
///
/// A cursor beyond `issued` — the highest cursor the service has handed out — is
/// refused too, so a well-formed token nobody was ever given names no page.
pub fn parse_page_token(token: &str, key: &EnvironmentKey, issued: u64) -> Option<u64> {
    let digits = token.strip_prefix(PAGE_TOKEN_PREFIX)?;
    // Both halves are hex digits, so refusing anything else here is what makes
    // the split below land on a character boundary: a request-supplied token is
    // arbitrary UTF-8, and splitting one at a byte offset inside a multi-byte
    // scalar would panic.
    if digits.len() != PAGE_TOKEN_SCOPE_WIDTH + PAGE_TOKEN_HEX_WIDTH
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let (scope, cursor) = digits.split_at(PAGE_TOKEN_SCOPE_WIDTH);
    if u32::from_str_radix(scope, 16).ok()? != token_scope(key) {
        return None;
    }
    let cursor = u64::from_str_radix(cursor, 16).ok()?;
    (cursor <= issued).then_some(cursor)
}

/// The app environment half of a page token: a digest of the (appId,
/// environmentName) the page was served for.
///
/// A token is only meaningful within the run that issued it, so the standard
/// hasher is enough; nothing persists a token across runs.
fn token_scope(key: &EnvironmentKey) -> u32 {
    use std::hash::{Hash, Hasher};

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.0.hash(&mut hasher);
    key.1.hash(&mut hasher);
    (hasher.finish() >> 32) as u32
}

/// True when `name` is a feature this service's metadata holds.
pub fn is_known_metadata_feature(name: &str) -> bool {
    METADATA_FEATURES.contains(&name)
}

/// The `ResourceNotFoundException` payload for an id that names no component in
/// the app environment the request addressed.
pub fn component_not_found(app_id: &str, environment_name: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "no component {id} in app {app_id} environment {environment_name}"
        )),
    }
}

/// The `InvalidParameterException` payload for an id that names a component
/// another app environment owns: the request is addressed to the wrong scope,
/// which is a different failure from an id that names nothing.
pub fn component_wrong_scope(
    app_id: &str,
    environment_name: &str,
    id: &str,
) -> InvalidParameterException {
    InvalidParameterException {
        message: Some(format!(
            "component {id} does not belong to app {app_id} environment {environment_name}"
        )),
    }
}

/// The `ResourceConflictException` payload for a component name another live
/// component of the same app environment already holds.
pub fn duplicate_component_name(
    app_id: &str,
    environment_name: &str,
    name: &str,
) -> ResourceConflictException {
    ResourceConflictException {
        message: Some(format!(
            "a component named {name} already exists in app {app_id} environment \
             {environment_name}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for an id that names no codegen job in
/// the app environment the request addressed.
pub fn codegen_job_not_found(
    app_id: &str,
    environment_name: &str,
    id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "no codegen job {id} in app {app_id} environment {environment_name}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for an id that names no form in the
/// app environment the request addressed.
pub fn form_not_found(app_id: &str, environment_name: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "no form {id} in app {app_id} environment {environment_name}"
        )),
    }
}

/// The `ResourceNotFoundException` payload for an id that names no theme in the
/// app environment the request addressed.
pub fn theme_not_found(app_id: &str, environment_name: &str, id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "no theme {id} in app {app_id} environment {environment_name}"
        )),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAmplifyuibuilderAmplifyuibuilderState>)
}
