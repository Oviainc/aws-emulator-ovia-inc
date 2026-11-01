// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-imagebuilder-imagebuilder service.
//!
//! The first resource this store holds is the **infrastructure configuration**: the
//! environment Image Builder builds and tests an image in. Every configuration is
//! stored exactly once, keyed by its own ARN, so `CreateInfrastructureConfiguration`
//! writes the record that `GetInfrastructureConfiguration` reads back.
//!
//! Two facts about that resource shape the store:
//!
//! * The ARN is derived from the NAME, not minted independently — the modelled
//!   `name` member documents "Image Builder generates the infrastructure
//!   configuration ARN from a normalized form of the name, so names that differ only
//!   in case, spaces, or underscores count as the same name", and
//!   "Infrastructure configuration names must be unique to your account in each
//!   Amazon Web Services Region". [`normalized_name`] is that one normalization, so
//!   the uniqueness test and the ARN agree by construction.
//! * The service is REGIONAL (its endpoints are `imagebuilder.<region>.<dns-suffix>`),
//!   so a configuration belongs to the Region its ARN names and a regional endpoint
//!   reaches only the configurations of its own Region
//!   ([`ComAmazonawsImagebuilderImagebuilderState::infrastructure_configuration`]).
//!
//! Beside it the store holds the two authoring resources an image build starts from.
//! A **component** is the YAML document Image Builder runs on a build or test instance,
//! and an **image recipe** names the parent image a build starts from together with the
//! components it applies. Three facts about them shape how they are stored:
//!
//! * a component is versioned twice over. `CreateComponent`'s `name` member documents
//!   "If a component with the same name and semantic version already exists in your
//!   account in the same Amazon Web Services Region, the request creates a new build
//!   version for it", so a record is keyed by its BUILD version ARN
//!   (`component/<name>/<version>/<build>`) and nothing is ever overwritten.
//! * a reference to a component may name a build version, or a version whose trailing
//!   nodes are `x` wildcards — "Version ARNs resolve to the latest available matching
//!   component build version" (`GetComponent`'s `componentBuildVersionArn`). That one
//!   resolution ([`ComAmazonawsImagebuilderImagebuilderState::resolve_component`])
//!   serves the read, the recipe's component list and the delete's dependency check.
//! * an image recipe is keyed by its own ARN, which carries the [`normalized_name`] and
//!   the semantic version, so one name+version pair is stored at most once.
//!
//! `clientToken` is an idempotency token: a repeat of a request that already
//! succeeded answers with the ORIGINAL response, and the same token carrying
//! different creation parameters is a mismatch. Both need the first request's
//! parameters and its answer, so every successful create files a [`CreateReceipt`]
//! beside the record, under the operation that earned it.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::ops::types::{
    AdditionalInstanceConfiguration, Component, ComponentConfiguration, ComponentParameterDetail,
    ComponentState, CreateComponentInput, CreateImageRecipeInput,
    CreateInfrastructureConfigurationInput, Filter, ImageRecipe, ImageRecipeSummary,
    InfrastructureConfiguration, InstanceBlockDeviceMapping, InstanceMetadataOptions, Logging,
    ParentInstanceConfiguration, Placement,
};

/// This service's ARN namespace and SigV4 signing name (`arnNamespace` / `sigv4Name`
/// of `smithy_query(query="service_info")`).
pub const ARN_SERVICE: &str = "imagebuilder";

/// The region used when a request carries no SigV4 credential scope to read one from;
/// the emulator's documented default region.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The resource portion an infrastructure-configuration ARN carries before its name,
/// from the modelled `InfrastructureConfigurationArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):infrastructure-configuration/[a-z0-9-_]+$`.
pub const INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX: &str = "infrastructure-configuration/";

/// The resource portion a component ARN carries before its name, from the modelled
/// `ComponentBuildVersionArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws(?:-[a-z-]+)?|third-party):component/[a-z0-9-_]+/[0-9]+\.[0-9]+\.[0-9]+/[0-9]+$`.
pub const COMPONENT_ARN_PREFIX: &str = "component/";

/// The resource portion an image-recipe ARN carries before its name, from the modelled
/// `ImageRecipeArn` pattern
/// `^arn:aws[^:]*:imagebuilder:[^:]+:(?:[0-9]{12}|aws):image-recipe/[a-z0-9-_]+/(?:[0-9]+|x)\.(?:[0-9]+|x)\.(?:[0-9]+|x)$`.
pub const IMAGE_RECIPE_ARN_PREFIX: &str = "image-recipe/";

/// The operations whose `clientToken` receipts share one index, each named as the key
/// that index files its receipts under — the operation's own name, so two operations
/// never read each other's.
pub const CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION: &str =
    "CreateInfrastructureConfiguration";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_COMPONENT_OPERATION: &str = "CreateComponent";

/// See [`CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION`].
pub const CREATE_IMAGE_RECIPE_OPERATION: &str = "CreateImageRecipe";

/// The listing a page token belongs to, so a token issued by one list operation cannot
/// be spent on another ([`encode_page_token`]).
pub const LIST_IMAGE_RECIPES_OPERATION: &str = "ListImageRecipes";

/// How many nodes of a semantic version a caller assigns.
///
/// From `semanticVersion`'s own documentation, which `CreateComponent` and
/// `CreateImageRecipe` repeat word for word: "The semantic version has four nodes:
/// <major>.<minor>.<patch>/<build>. You can assign values for the first three", and
/// "Image Builder automatically assigns the build number to the fourth node".
pub const ASSIGNED_VERSION_NODES: usize = 3;

/// The largest value an assigned semantic-version node may carry: "The upper limit is
/// 2^30-1, or 1073741823, for each node" (`semanticVersion`).
pub const MAX_VERSION_NODE: u64 = 1_073_741_823;

/// The build number the FIRST build version of one component name+version pair carries;
/// Image Builder counts the fourth node from one.
pub const FIRST_BUILD_VERSION: u64 = 1;

/// The status a stored component reports, from the modelled `ComponentStatus` enum
/// (`ACTIVE`, `DEPRECATED`, `DISABLED`): a component this service just stored is the one
/// a new recipe may include, which is what `ACTIVE` means — "Components with a status of
/// DEPRECATED or DISABLED can't be added to new recipes".
pub const COMPONENT_STATUS_ACTIVE: &str = "ACTIVE";

/// The component type of a document that is anything but a lone `test` phase, from
/// `CreateComponent`'s own documentation: "Image Builder determines the component type
/// from the document. If the document contains a single phase named test, the component
/// type is TEST. Otherwise, the component type is BUILD."
pub const COMPONENT_TYPE_BUILD: &str = "BUILD";

/// The component type of a document that is a single phase named `test` — the other
/// value of the modelled `ComponentType` enum. See [`COMPONENT_TYPE_BUILD`].
pub const COMPONENT_TYPE_TEST: &str = "TEST";

/// Whether a stored component's data is encrypted at rest, from the modelled `encrypted`
/// member: "Image Builder encrypts all component data at rest, so this value is always
/// true. This field is retained for backward compatibility."
pub const COMPONENT_DATA_ENCRYPTED: bool = true;

/// The scheme a component document URI must carry, from `CreateComponent`'s `uri`: "This
/// must be an S3 URL (s3://bucket/key)".
pub const COMPONENT_URI_SCHEME: &str = "s3://";

/// The ownership a caller's own resources carry, from the modelled `Ownership` enum.
/// `ListImageRecipes`' `owner` documents "By default, this request will only show image
/// recipes owned by your account", so this is also the value an omitted `owner` means.
pub const OWNERSHIP_SELF: &str = "Self";

/// The filter names `ListImageRecipes` accepts, from the `filters` member's own list:
/// "Use the following filters to streamline results: name, parentImage, platform."
pub const IMAGE_RECIPE_FILTER_NAMES: &[&str] = &["name", "parentImage", "platform"];

/// The most items one `ListImageRecipes` page carries when the request names no
/// `maxResults`: the modelled `maxResults` range's own maximum (1..25).
pub const MAX_IMAGE_RECIPES_PER_PAGE: usize = 25;

/// The most tags one resource carries.
///
/// The modelled `TagMap`'s own length bound (`len 1..50`) — and `TagMap` is the target of
/// every `tags` member, the one a resource reports as much as the one a request submits, so
/// it bounds the map a resource ends up holding and not only the map one call carries.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// The most components one image recipe includes.
///
/// The documented quota, from the modelled `components` member of `ImageRecipe`: "A recipe
/// can contain a maximum of 20 build and test components in any combination, by default.
/// This maximum is an adjustable quota." The modelled list carries no maximum of its own,
/// so the quota is read here — which is also what bounds the work ONE create spends
/// resolving the references it names.
pub const MAX_RECIPE_COMPONENTS: usize = 20;

// ---------------------------------------------------------------------------
// Pre-flight bounds on a caller-supplied component document
//
// `data` is caller-supplied text handed to a RECURSIVE YAML loader, so how deep that
// loader may descend has to be refused BEFORE it is called: an `EventReceiver` cannot
// abort `load` (`on_event` returns `()`), and exhausting the stack aborts the process
// rather than unwinding. The modelled `len 1..16000` bounds how BIG a document is, not how
// deeply it nests, so each measure below is a bound on NESTING and a large flat document
// trips none of them.
// ---------------------------------------------------------------------------

/// The most flow collections (`[`, `{`) a component document may have OPEN at once,
/// closers subtracted: the nesting a well-formed document reaches. Chosen: far past the
/// nesting of a document's `phases` → `steps` → `inputs` tree.
pub const MAX_DOCUMENT_FLOW_DEPTH: usize = 64;

/// The most flow openers (`[`, `{`) a component document may carry in total.
///
/// The backstop for [`MAX_DOCUMENT_FLOW_DEPTH`]: this is a byte scan, so a `]` inside a
/// quoted scalar lowers the running depth while the loader's own depth keeps climbing —
/// but no document can nest deeper than its opener COUNT. Chosen as a depth the loader
/// survives with room to spare, and far above the flow collections a real document carries
/// in total: the modelled `data` maximum is 16,000 bytes, and a document carrying this
/// many flow collections inside that is almost all punctuation.
pub const MAX_DOCUMENT_FLOW_OPENERS: usize = 2_048;

/// The most leading spaces one line of a component document may carry, which is what block
/// nesting costs per level. Chosen: far past the indentation of a document written at any
/// sane indent step.
pub const MAX_DOCUMENT_INDENT: usize = 256;

/// The most compact block indicators (`- `, `? `) one line of a component document may
/// carry, which is where block nesting (`- - - x`) descends without costing a column. A
/// hyphen inside a name, a command or a description is not one of these (no space follows
/// it). Chosen.
pub const MAX_DOCUMENT_COMPACT_PER_LINE: usize = 256;

/// Applied when `CreateInfrastructureConfiguration` omits
/// `terminateInstanceOnFailure`, whose own documentation says "Defaults to true".
pub const DEFAULT_TERMINATE_INSTANCE_ON_FAILURE: bool = true;

/// Applied when `CreateInfrastructureConfiguration` omits
/// `terminateInstanceOnCompletion`, whose own documentation says "Defaults to true".
pub const DEFAULT_TERMINATE_INSTANCE_ON_COMPLETION: bool = true;

/// The metadata-tag keys a caller may not assign to the build and test instances.
///
/// From the `resourceTags` member's own documentation: "Tag keys can't begin with
/// `aws:` or match one of the following reserved keys: CreatedBy, Ec2ImageBuilderArn,
/// Name, or Tags." The `aws:` prefix is already refused by the modelled `TagKey`
/// pattern before a handler runs; these four are not, so they are refused here.
pub const RESERVED_RESOURCE_TAG_KEYS: &[&str] =
    &["CreatedBy", "Ec2ImageBuilderArn", "Name", "Tags"];

/// The most infrastructure configurations this emulator will hold.
///
/// An emulator resource guard, NOT an AWS quota: Image Builder publishes a per-account
/// limit but the model carries no value for it, so this cap is set well above any
/// realistic use and exists only so a caller-driven store cannot grow without bound.
pub const MAX_INFRASTRUCTURE_CONFIGURATIONS: usize = 1_000;

/// The most components this emulator will hold, counting every build version.
///
/// An emulator resource guard like [`MAX_INFRASTRUCTURE_CONFIGURATIONS`], not an AWS
/// quota: the model carries no value for Image Builder's published per-account limit, so
/// this cap sits well above any realistic use and exists only so a caller-driven store
/// cannot grow without bound.
pub const MAX_COMPONENTS: usize = 1_000;

/// The most image recipes this emulator will hold. An emulator resource guard, for the
/// same reason as [`MAX_COMPONENTS`].
pub const MAX_IMAGE_RECIPES: usize = 1_000;

/// The most `clientToken` receipts this emulator keeps.
///
/// The receipt index's OWN bound, stated here rather than borrowed from the resource caps
/// above: a receipt is read only while the caller that earned it is still retrying, and
/// nothing else ties its lifetime to the resource it names — so where a delete removes a
/// resource without removing the receipt that named it, THIS is the constant that still
/// bounds the index. Past it the OLDEST receipt is evicted
/// ([`ComAmazonawsImagebuilderImagebuilderState::file_create_receipt`]), never the create.
pub const MAX_CREATE_RECEIPTS: usize =
    2 * (MAX_INFRASTRUCTURE_CONFIGURATIONS + MAX_COMPONENTS + MAX_IMAGE_RECIPES);

/// One stored infrastructure configuration: what a create wrote, which is what every
/// later read reports.
///
/// The list and map members are held as `Option`, so a read reports exactly what the
/// create stored — an omitted `instanceTypes` is reported as absent rather than as an
/// empty list.
#[derive(Debug, Clone)]
pub struct InfrastructureConfigurationRecord {
    /// This configuration's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub description: Option<String>,
    pub instance_types: Option<Vec<String>>,
    pub instance_profile_name: String,
    pub security_group_ids: Option<Vec<String>>,
    pub subnet_id: Option<String>,
    pub logging: Option<Logging>,
    pub key_pair: Option<String>,
    pub terminate_instance_on_failure: bool,
    pub terminate_instance_on_completion: bool,
    pub sns_topic_arn: Option<String>,
    pub resource_tags: Option<BTreeMap<String, String>>,
    pub instance_metadata_options: Option<InstanceMetadataOptions>,
    pub tags: Option<BTreeMap<String, String>>,
    pub placement: Option<Placement>,
    pub parent_instance: Option<ParentInstanceConfiguration>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    /// When it was last written. The create IS the last write until an update
    /// replaces it, so a freshly created configuration reports its creation instant.
    pub date_updated: String,
}

impl InfrastructureConfigurationRecord {
    /// This record as the modelled `InfrastructureConfiguration` a read reports.
    #[must_use]
    pub fn view(&self) -> InfrastructureConfiguration {
        InfrastructureConfiguration {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            description: self.description.clone(),
            instance_types: self.instance_types.clone(),
            instance_profile_name: Some(self.instance_profile_name.clone()),
            security_group_ids: self.security_group_ids.clone(),
            subnet_id: self.subnet_id.clone(),
            logging: self.logging.clone(),
            key_pair: self.key_pair.clone(),
            terminate_instance_on_failure: Some(self.terminate_instance_on_failure),
            sns_topic_arn: self.sns_topic_arn.clone(),
            date_created: Some(self.date_created.clone()),
            date_updated: Some(self.date_updated.clone()),
            resource_tags: self.resource_tags.clone().map(into_tag_map),
            instance_metadata_options: self.instance_metadata_options.clone(),
            tags: self.tags.clone().map(into_tag_map),
            placement: self.placement.clone(),
            terminate_instance_on_completion: Some(self.terminate_instance_on_completion),
            parent_instance: self.parent_instance.clone(),
        }
    }
}

/// One stored component build version: what a create wrote, which is what every later
/// read reports.
#[derive(Debug, Clone)]
pub struct ComponentRecord {
    /// This build version's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    /// The three assigned version nodes, as the caller submitted them.
    pub version: String,
    /// The fourth node, which Image Builder assigns: which build of this name+version
    /// pair this is, counted from [`FIRST_BUILD_VERSION`].
    pub build_version: u64,
    pub description: Option<String>,
    pub change_description: Option<String>,
    /// `BUILD` or `TEST`, derived from the document's phases rather than submitted.
    pub component_type: String,
    pub platform: String,
    pub supported_os_versions: Option<Vec<String>>,
    /// The account that owns it — "The value is your account ID for components that you
    /// own" (the modelled `owner` member).
    pub owner: String,
    /// The inline document the create carried, when it carried one.
    pub data: Option<String>,
    /// The document URI the create carried instead, when it carried one.
    pub uri: Option<String>,
    pub kms_key_id: Option<String>,
    /// The parameters the document declared, derived from it at create time.
    pub parameters: Option<Vec<ComponentParameterDetail>>,
    /// When it was created, as the modelled string `DateTime` carries an instant.
    pub date_created: String,
    pub tags: Option<BTreeMap<String, String>>,
}

impl ComponentRecord {
    /// This record as the modelled `Component` a read reports.
    #[must_use]
    pub fn view(&self) -> Component {
        Component {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            version: Some(self.version.clone()),
            description: self.description.clone(),
            change_description: self.change_description.clone(),
            r#type: Some(self.component_type.clone()),
            platform: Some(self.platform.clone()),
            supported_os_versions: self.supported_os_versions.clone(),
            state: Some(ComponentState {
                status: Some(COMPONENT_STATUS_ACTIVE.to_string()),
                // `reason` "Describes how or why the component changed state"; this one
                // has never changed state, so there is nothing to describe.
                reason: None,
            }),
            parameters: self.parameters.clone(),
            owner: Some(self.owner.clone()),
            data: self.data.clone(),
            kms_key_id: self.kms_key_id.clone(),
            encrypted: Some(COMPONENT_DATA_ENCRYPTED),
            date_created: Some(self.date_created.clone()),
            tags: self.tags.clone().map(into_tag_map),
            // "Contains the name of the publisher if this is a third-party component.
            // Otherwise, this property is empty" — a component this account created is
            // not third-party.
            publisher: None,
            // "Indicates whether component source is hidden from view" — the stored
            // document is reported, so it is not hidden.
            obfuscate: false,
            // "Contains product codes that are used for billing purposes for Amazon Web
            // Services Marketplace components" — this one came from a caller, not from
            // Marketplace.
            product_codes: None,
        }
    }

    /// The four version nodes this build version sorts by: the three assigned ones and
    /// the build number, so "the latest available matching component build version" is
    /// the greatest of them.
    #[must_use]
    pub fn version_order(&self) -> [u64; ASSIGNED_VERSION_NODES + 1] {
        let [major, minor, patch] = parse_semantic_version(&self.version).unwrap_or_default();
        [major, minor, patch, self.build_version]
    }
}

/// One stored image recipe: what a create wrote, which is what every later read reports.
#[derive(Debug, Clone)]
pub struct ImageRecipeRecord {
    /// This recipe's own ARN, which is also its key in the store.
    pub arn: String,
    /// The Region whose endpoint created it, and the only one that can reach it.
    pub region: String,
    /// The name as the caller submitted it; the ARN carries its [`normalized_name`].
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    /// The platform the recipe builds for, taken from the components it includes — a
    /// recipe that includes none has no platform to report.
    pub platform: Option<String>,
    pub owner: String,
    /// The component configurations, in the order the create submitted them.
    pub components: Option<Vec<ComponentConfiguration>>,
    pub parent_image: String,
    pub block_device_mappings: Option<Vec<InstanceBlockDeviceMapping>>,
    pub working_directory: Option<String>,
    pub additional_instance_configuration: Option<AdditionalInstanceConfiguration>,
    pub ami_tags: Option<BTreeMap<String, String>>,
    pub ami_watermarks: Option<Vec<String>>,
    pub tags: Option<BTreeMap<String, String>>,
    pub date_created: String,
}

impl ImageRecipeRecord {
    /// This record as the modelled `ImageRecipe` a read reports.
    #[must_use]
    pub fn view(&self) -> ImageRecipe {
        ImageRecipe {
            arn: Some(self.arn.clone()),
            // The modelled `type` member: "For an image recipe, this is always AMI. …
            // This field isn't currently returned in responses."
            r#type: None,
            name: Some(self.name.clone()),
            description: self.description.clone(),
            platform: self.platform.clone(),
            owner: Some(self.owner.clone()),
            version: Some(self.version.clone()),
            components: self.components.clone(),
            parent_image: Some(self.parent_image.clone()),
            block_device_mappings: self.block_device_mappings.clone(),
            date_created: Some(self.date_created.clone()),
            tags: self.tags.clone().map(into_tag_map),
            working_directory: self.working_directory.clone(),
            additional_instance_configuration: self.additional_instance_configuration.clone(),
            ami_tags: self.ami_tags.clone().map(into_tag_map),
            ami_watermarks: self.ami_watermarks.clone(),
        }
    }

    /// This record as the modelled `ImageRecipeSummary` one listed page entry carries.
    #[must_use]
    pub fn summary(&self) -> ImageRecipeSummary {
        ImageRecipeSummary {
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            platform: self.platform.clone(),
            owner: Some(self.owner.clone()),
            parent_image: Some(self.parent_image.clone()),
            date_created: Some(self.date_created.clone()),
            tags: self.tags.clone().map(into_tag_map),
        }
    }

    /// The value of the member `filter_name` filters on, or `None` when this record
    /// reports none. See [`IMAGE_RECIPE_FILTER_NAMES`].
    #[must_use]
    pub fn filterable(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            "name" => Some(self.name.as_str()),
            "parentImage" => Some(self.parent_image.as_str()),
            "platform" => self.platform.as_deref(),
            _ => None,
        }
    }
}

/// What a successful create filed about the request it served, so a retry of that
/// request can be told from a different one carrying the same `clientToken`.
#[derive(Debug, Clone)]
pub struct CreateReceipt {
    /// The creation parameters the original request carried (one of the
    /// `*_request_fingerprint` renderings below).
    pub fingerprint: String,
    /// The `requestId` the original response named.
    pub request_id: String,
    /// The `clientToken` the original response echoed.
    pub client_token: String,
    /// The ARN the original response named.
    pub arn: String,
    /// Which receipt this is, counted from one, so the OLDEST is the one the index
    /// evicts once it is full.
    pub filed: u64,
}

/// Which of this store's resource kinds an ARN names, once the ARN has been found to
/// name a stored resource.
///
/// The modelled `ImageBuilderArn` the tagging operations take admits ten resource kinds;
/// these are the ones this store holds, so they are the ones a tag can be read from or
/// written to. An ARN of any other kind names nothing stored and is reported missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResource {
    /// An infrastructure configuration ([`INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX`]).
    InfrastructureConfiguration,
    /// One component build version ([`COMPONENT_ARN_PREFIX`]).
    Component,
    /// An image recipe ([`IMAGE_RECIPE_ARN_PREFIX`]).
    ImageRecipe,
}

/// Per-service state for com-amazonaws-imagebuilder-imagebuilder. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsImagebuilderImagebuilderState {
    /// Every stored infrastructure configuration, keyed by its own ARN — which
    /// carries both its Region and its normalized name, so one key says where a
    /// configuration lives and which name is taken there.
    pub infrastructure_configurations: BTreeMap<String, InfrastructureConfigurationRecord>,
    /// Every stored component build version, keyed by its own build version ARN — which
    /// carries its Region, its normalized name, its version and its build number.
    pub components: BTreeMap<String, ComponentRecord>,
    /// Every stored image recipe, keyed by its own ARN — which carries its Region, its
    /// normalized name and its version, so one key says which name+version pair is
    /// taken where.
    pub image_recipes: BTreeMap<String, ImageRecipeRecord>,
    /// The answer each `clientToken` already got, keyed by (operation, Region, token):
    /// an idempotency token is spent on ONE operation, so the same token may create a
    /// component and a recipe. Bounded on its own, by age, at [`MAX_CREATE_RECEIPTS`] —
    /// see [`Self::file_create_receipt`].
    pub create_receipts: BTreeMap<(String, String, String), CreateReceipt>,
    /// How many receipts have been filed, which is what orders them by age.
    pub create_receipts_filed: u64,
}

impl ComAmazonawsImagebuilderImagebuilderState {
    /// The configuration `arn` names, but only when it belongs to `request_region`.
    ///
    /// A regional endpoint serves only its own Region's resources, so a configuration
    /// of another Region answers as if it did not exist.
    #[must_use]
    pub fn infrastructure_configuration(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<&InfrastructureConfigurationRecord> {
        self.infrastructure_configurations
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created configuration together with the receipt for the request
    /// that created it.
    pub fn insert_infrastructure_configuration(
        &mut self,
        record: InfrastructureConfigurationRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.infrastructure_configurations
            .insert(record.arn.clone(), record);
    }

    /// The receipt `client_token` already earned from `CreateInfrastructureConfiguration`
    /// in `request_region`, if any.
    #[must_use]
    pub fn infrastructure_configuration_receipt(
        &self,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipt(
            CREATE_INFRASTRUCTURE_CONFIGURATION_OPERATION,
            request_region,
            client_token,
        )
    }

    /// Whether the store is already holding as many configurations as this emulator
    /// will keep ([`MAX_INFRASTRUCTURE_CONFIGURATIONS`]).
    #[must_use]
    pub fn infrastructure_configuration_store_is_full(&self) -> bool {
        self.infrastructure_configurations.len() >= MAX_INFRASTRUCTURE_CONFIGURATIONS
    }

    /// The receipt `client_token` already earned from `operation` in `request_region`,
    /// if any.
    #[must_use]
    pub fn create_receipt(
        &self,
        operation: &str,
        request_region: &str,
        client_token: &str,
    ) -> Option<&CreateReceipt> {
        self.create_receipts.get(&(
            operation.to_string(),
            request_region.to_string(),
            client_token.to_string(),
        ))
    }

    /// File the receipt for a request that just created `arn`.
    ///
    /// The receipt index is bounded by AGE rather than by refusing the create: a
    /// receipt is only ever read by a caller still retrying its own request, so past
    /// [`MAX_CREATE_RECEIPTS`] — the index's own bound — the oldest is dropped. A
    /// retry whose receipt was evicted is then answered as a fresh request — which the
    /// uniqueness rule of the resource it names refuses, the same answer any caller
    /// asking for a name already taken gets — and the index can never wedge a create.
    pub fn file_create_receipt(
        &mut self,
        operation: &str,
        region: &str,
        client_token: &str,
        fingerprint: String,
        request_id: String,
        arn: String,
    ) {
        self.create_receipts_filed = self.create_receipts_filed.saturating_add(1);
        let receipt = CreateReceipt {
            fingerprint,
            request_id,
            client_token: client_token.to_string(),
            arn,
            filed: self.create_receipts_filed,
        };
        while self.create_receipts.len() >= MAX_CREATE_RECEIPTS {
            let oldest = self
                .create_receipts
                .iter()
                .min_by_key(|(_, filed)| filed.filed)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.create_receipts.remove(&key);
                }
                None => break,
            }
        }
        self.create_receipts.insert(
            (
                operation.to_string(),
                region.to_string(),
                client_token.to_string(),
            ),
            receipt,
        );
    }

    /// The component build version the ARN `reference` names, when one is stored in
    /// `request_region`.
    ///
    /// `reference` may be a build version ARN or a version ARN whose trailing nodes are
    /// `x` wildcards: "Version ARNs resolve to the latest available matching component
    /// build version" (`GetComponent`'s `componentBuildVersionArn`), so the greatest
    /// matching build version is the one answered. `None` when `reference` is not a
    /// component ARN of this service, names another Region, or matches nothing stored.
    #[must_use]
    pub fn resolve_component(
        &self,
        reference: &str,
        request_region: &str,
    ) -> Option<&ComponentRecord> {
        let reference = ComponentReference::parse(reference)?;
        if reference.region != request_region {
            return None;
        }
        self.components
            .values()
            .filter(|record| reference.matches(record))
            .max_by_key(|record| record.version_order())
    }

    /// The build number the NEXT build version of this name+version pair carries.
    ///
    /// "If a component with the same name and semantic version already exists in your
    /// account in the same Amazon Web Services Region, the request creates a new build
    /// version for it" (`CreateComponent`'s `name`), so the fourth node counts up from
    /// the greatest build already stored, and starts at [`FIRST_BUILD_VERSION`].
    #[must_use]
    pub fn next_component_build_version(
        &self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> u64 {
        self.component_builds(region, normalized, version)
            .into_iter()
            .map(|record| record.build_version)
            .max()
            .map_or(FIRST_BUILD_VERSION, |latest| latest.saturating_add(1))
    }

    /// The build version of this name+version pair with the greatest build number, if
    /// any is stored.
    #[must_use]
    pub fn latest_component_build(
        &self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> Option<&ComponentRecord> {
        self.component_builds(region, normalized, version)
            .into_iter()
            .max_by_key(|record| record.build_version)
    }

    /// Every stored build version of one component name+version pair in one Region.
    fn component_builds<'store>(
        &'store self,
        region: &str,
        normalized: &str,
        version: &str,
    ) -> Vec<&'store ComponentRecord> {
        self.components
            .values()
            .filter(|record| {
                record.region == region
                    && normalized_name(&record.name) == normalized
                    && record.version == version
            })
            .collect()
    }

    /// Store a newly created component build version together with the receipt for the
    /// request that created it.
    pub fn insert_component(
        &mut self,
        record: ComponentRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_COMPONENT_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.components.insert(record.arn.clone(), record);
    }

    /// Whether the store is already holding as many components as this emulator will
    /// keep ([`MAX_COMPONENTS`]).
    #[must_use]
    pub fn component_store_is_full(&self) -> bool {
        self.components.len() >= MAX_COMPONENTS
    }

    /// The first stored recipe whose component list reaches the component `arn` names,
    /// if any does.
    ///
    /// A recipe reaches a build version whenever one of its references MATCHES it, not
    /// only when it is the newest match: a wildcard reference answers with whichever
    /// build is newest at the time, so every build it matches is still in use.
    #[must_use]
    pub fn recipe_depending_on_component(&self, arn: &str) -> Option<&ImageRecipeRecord> {
        let component = self.components.get(arn)?;
        self.image_recipes.values().find(|recipe| {
            recipe
                .components
                .iter()
                .flatten()
                .filter_map(|configuration| {
                    ComponentReference::parse(&configuration.component_arn)
                })
                .any(|reference| reference.matches(component))
        })
    }

    /// Forget the component `arn` names, when it belongs to `request_region`.
    pub fn remove_component(&mut self, arn: &str, request_region: &str) -> Option<ComponentRecord> {
        self.component(arn, request_region)?;
        self.components.remove(arn)
    }

    /// The component build version `arn` names exactly, but only when it belongs to
    /// `request_region`.
    #[must_use]
    pub fn component(&self, arn: &str, request_region: &str) -> Option<&ComponentRecord> {
        self.components
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// The image recipe `arn` names, but only when it belongs to `request_region`.
    #[must_use]
    pub fn image_recipe(&self, arn: &str, request_region: &str) -> Option<&ImageRecipeRecord> {
        self.image_recipes
            .get(arn)
            .filter(|record| record.region == request_region)
    }

    /// Store a newly created image recipe together with the receipt for the request that
    /// created it.
    pub fn insert_image_recipe(
        &mut self,
        record: ImageRecipeRecord,
        client_token: &str,
        fingerprint: String,
        request_id: String,
    ) {
        self.file_create_receipt(
            CREATE_IMAGE_RECIPE_OPERATION,
            &record.region,
            client_token,
            fingerprint,
            request_id,
            record.arn.clone(),
        );
        self.image_recipes.insert(record.arn.clone(), record);
    }

    /// Forget the image recipe `arn` names, when it belongs to `request_region`.
    pub fn remove_image_recipe(
        &mut self,
        arn: &str,
        request_region: &str,
    ) -> Option<ImageRecipeRecord> {
        self.image_recipe(arn, request_region)?;
        self.image_recipes.remove(arn)
    }

    /// Whether the store is already holding as many image recipes as this emulator will
    /// keep ([`MAX_IMAGE_RECIPES`]).
    #[must_use]
    pub fn image_recipe_store_is_full(&self) -> bool {
        self.image_recipes.len() >= MAX_IMAGE_RECIPES
    }

    /// Which stored resource `arn` names, when one of this Region's resources carries that
    /// ARN exactly.
    ///
    /// A tagging call names its resource by the resource's OWN ARN, so the lookup is exact:
    /// a component ARN answers the build version it names, never the latest build of a
    /// version it resolves to. A regional endpoint reaches only its own Region's
    /// resources, which each lookup below already enforces.
    #[must_use]
    pub fn tagged_resource(&self, arn: &str, request_region: &str) -> Option<TaggedResource> {
        if self.infrastructure_configuration(arn, request_region).is_some() {
            return Some(TaggedResource::InfrastructureConfiguration);
        }
        if self.component(arn, request_region).is_some() {
            return Some(TaggedResource::Component);
        }
        if self.image_recipe(arn, request_region).is_some() {
            return Some(TaggedResource::ImageRecipe);
        }
        None
    }

    /// The tags the resource `arn` names currently carries, or `None` when nothing in
    /// this Region carries that ARN.
    ///
    /// Named for the resource's own `tags` member, which is what a tagging call reads and
    /// writes — NOT the `resourceTags` an infrastructure configuration puts on its build
    /// and test instances, which is a different member of one record.
    ///
    /// A stored resource that was never tagged carries NO tags, which is an empty map
    /// rather than an absent resource — the two are different answers.
    #[must_use]
    pub fn tags_of_resource(
        &self,
        arn: &str,
        request_region: &str,
    ) -> Option<BTreeMap<String, String>> {
        let stored = match self.tagged_resource(arn, request_region)? {
            TaggedResource::InfrastructureConfiguration => self
                .infrastructure_configurations
                .get(arn)
                .and_then(|record| record.tags.clone()),
            TaggedResource::Component => {
                self.components.get(arn).and_then(|record| record.tags.clone())
            }
            TaggedResource::ImageRecipe => self
                .image_recipes
                .get(arn)
                .and_then(|record| record.tags.clone()),
        };
        Some(stored.unwrap_or_default())
    }

    /// The tag map of the resource `arn` names, to write through, or `None` when nothing
    /// in this Region carries that ARN.
    ///
    /// Hands back the record's own `tags` member, so a tag written here is the tag every
    /// later read of that resource reports — whether the read is a tag listing or the
    /// resource's own Get.
    pub fn tags_of_resource_mut(
        &mut self,
        arn: &str,
        request_region: &str,
    ) -> Option<&mut Option<BTreeMap<String, String>>> {
        match self.tagged_resource(arn, request_region)? {
            TaggedResource::InfrastructureConfiguration => self
                .infrastructure_configurations
                .get_mut(arn)
                .map(|record| &mut record.tags),
            TaggedResource::Component => {
                self.components.get_mut(arn).map(|record| &mut record.tags)
            }
            TaggedResource::ImageRecipe => {
                self.image_recipes.get_mut(arn).map(|record| &mut record.tags)
            }
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsImagebuilderImagebuilderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsImagebuilderImagebuilderState>)
}

/// The normalized form of a resource name: the form its ARN carries, and the form two
/// names are compared in.
///
/// The rule is the same for every resource this store holds, and each one's own `name`
/// member states it: "Image Builder generates the infrastructure configuration ARN from a
/// normalized form of the name, so names that differ only in case, spaces, or
/// underscores count as the same name", and `CreateComponent`'s says the same of a
/// component. Case folds down, and a space and an underscore are the same separator as a
/// hyphen — which is what makes those three forms one name. The modelled `ResourceName`
/// pattern admits only `[-_A-Za-z0-9 ]`, so the result is always `[a-z0-9-]+` and
/// therefore inside the `[a-z0-9-_]+` every one of those ARN patterns allows.
#[must_use]
pub fn normalized_name(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            other => other.to_ascii_lowercase(),
        })
        .collect()
}

/// The ARN of the infrastructure configuration `name` names in `region`.
#[must_use]
pub fn infrastructure_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{INFRASTRUCTURE_CONFIGURATION_ARN_PREFIX}{}",
            normalized_name(name)
        ),
    )
}

/// The ARN of one build version of the component `name` names at `version` in `region`.
#[must_use]
pub fn component_build_version_arn(
    region: &str,
    name: &str,
    version: &str,
    build_version: u64,
) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{COMPONENT_ARN_PREFIX}{}/{version}/{build_version}",
            normalized_name(name)
        ),
    )
}

/// The ARN of the image recipe `name` names at `version` in `region`.
#[must_use]
pub fn image_recipe_arn(region: &str, name: &str, version: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "{IMAGE_RECIPE_ARN_PREFIX}{}/{version}",
            normalized_name(name)
        ),
    )
}

/// The three assigned nodes `version` carries, or `None` when it is not three integers
/// within the documented upper limit.
///
/// The modelled pattern admits any digits at all, so the bound
/// `semanticVersion` documents — "you can assign any positive integer value, including
/// zero. The upper limit is 2^30-1, or 1073741823, for each node" — is read here, and a
/// version past it is out of bounds rather than merely long.
#[must_use]
pub fn parse_semantic_version(version: &str) -> Option<[u64; ASSIGNED_VERSION_NODES]> {
    let mut nodes = [0_u64; ASSIGNED_VERSION_NODES];
    let mut submitted = version.split('.');
    for node in &mut nodes {
        let field = submitted.next()?;
        let value: u64 = field.parse().ok()?;
        if value > MAX_VERSION_NODE {
            return None;
        }
        *node = value;
    }
    submitted.next().is_none().then_some(nodes)
}

/// One node of a version a REFERENCE carries: an assigned value, or the `x` wildcard
/// that matches any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionNode {
    /// The `x` wildcard: "The version can use the `x` wildcard in trailing positions"
    /// (`GetComponent`'s `componentBuildVersionArn`).
    Any,
    /// One assigned value.
    Exact(u64),
}

impl VersionNode {
    /// Whether this node admits the assigned `value`.
    #[must_use]
    pub fn admits(self, value: u64) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(exact) => exact == value,
        }
    }
}

/// What a component ARN names: everything the resolution rule compares a stored build
/// version against.
///
/// The ONE reading of a component reference, so the read, a recipe's component list and
/// the delete's dependency check agree on which build versions one ARN reaches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentReference {
    /// The Region field of the ARN.
    pub region: String,
    /// The normalized name the ARN carries.
    pub name: String,
    /// The three version nodes, each assigned or wildcard.
    pub version: [VersionNode; ASSIGNED_VERSION_NODES],
    /// The build number, when the ARN names one — a build version ARN does, a version
    /// ARN does not.
    pub build_version: Option<u64>,
}

impl ComponentReference {
    /// What `value` names, or `None` when it is not a component ARN of this service.
    ///
    /// The ARN's six fields come from this tree's one ARN parse
    /// (`service_common::arn`); what is read here is only the resource portion
    /// `component/<name>/<version>[/<build>]`. A wildcard is admitted only in a TRAILING
    /// position, as `GetComponent`'s `componentBuildVersionArn` documents (`1.0.x`,
    /// `1.x.x`), so `x.0.0` names nothing.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        let arn = ::service_common::arn::parse_of(value, ARN_SERVICE)?;
        let resource = arn.resource.strip_prefix(COMPONENT_ARN_PREFIX)?;
        let mut fields = resource.split('/');
        let name = fields.next()?;
        let version = fields.next()?;
        let build_version = match fields.next() {
            Some(build) => Some(build.parse().ok()?),
            None => None,
        };
        if fields.next().is_some() || name.is_empty() {
            return None;
        }

        let mut nodes = [VersionNode::Any; ASSIGNED_VERSION_NODES];
        let mut submitted = version.split('.');
        for node in &mut nodes {
            *node = match submitted.next()? {
                "x" => VersionNode::Any,
                assigned => VersionNode::Exact(
                    assigned
                        .parse()
                        .ok()
                        .filter(|value| *value <= MAX_VERSION_NODE)?,
                ),
            };
        }
        if submitted.next().is_some() {
            return None;
        }
        // Trailing positions only: once a node is a wildcard, every later one is too.
        let wildcard_from = nodes
            .iter()
            .position(|node| *node == VersionNode::Any)
            .unwrap_or(ASSIGNED_VERSION_NODES);
        if nodes
            .iter()
            .skip(wildcard_from)
            .any(|node| *node != VersionNode::Any)
        {
            return None;
        }
        // A build version ARN names an exact version: `1.x.x/2` reaches no single build.
        if build_version.is_some() && wildcard_from != ASSIGNED_VERSION_NODES {
            return None;
        }

        Some(Self {
            region: arn.region.to_string(),
            name: name.to_string(),
            version: nodes,
            build_version,
        })
    }

    /// Whether this reference reaches the stored build version `record`.
    #[must_use]
    pub fn matches(&self, record: &ComponentRecord) -> bool {
        if self.region != record.region || self.name != normalized_name(&record.name) {
            return false;
        }
        if self
            .build_version
            .is_some_and(|build| build != record.build_version)
        {
            return false;
        }
        let Some(assigned) = parse_semantic_version(&record.version) else {
            return false;
        };
        self.version
            .iter()
            .zip(assigned)
            .all(|(node, value)| node.admits(value))
    }
}

// ---------------------------------------------------------------------------
// The component document
//
// "Creates a new component that can be used to build, validate, test, and assess your
// image. The component is based on a YAML document that you specify using exactly one of
// the following methods: Inline, using the data property in the request body. A URL that
// points to a YAML document file stored in Amazon S3, using the uri property in the
// request body. Image Builder determines the component type from the document. If the
// document contains a single phase named test, the component type is TEST. Otherwise,
// the component type is BUILD." (`CreateComponent`'s own documentation.)
//
// So what a stored component derives from the document it carries is read HERE, once, by
// a maintained YAML 1.2 parser.
// ---------------------------------------------------------------------------

/// The document key the phases are listed under, which is what the component type is
/// read from.
const PHASES_KEY: &str = "phases";

/// The key one phase's name is read from.
const PHASE_NAME_KEY: &str = "name";

/// The phase name whose LONE presence makes a document a test component.
const TEST_PHASE_NAME: &str = "test";

/// The document key the component's parameters are listed under.
const PARAMETERS_KEY: &str = "parameters";

/// The keys one declared parameter carries.
const PARAMETER_TYPE_KEY: &str = "type";
const PARAMETER_DEFAULT_KEY: &str = "default";
const PARAMETER_DESCRIPTION_KEY: &str = "description";

/// What a component document declares: everything a stored component derives from it.
#[derive(Debug, Clone)]
pub struct ComponentDocument {
    /// `BUILD` or `TEST`, from the document's phases.
    pub component_type: String,
    /// The parameters the document defined, in the order it listed them — the modelled
    /// `parameters` member "Contains parameter details for each of the parameters that
    /// the component document defined for the component".
    pub parameters: Option<Vec<ComponentParameterDetail>>,
}

/// What `data` declares, or why it is not a component document.
///
/// The `Err` is the wording the refusal carries: this service's own statement of what a
/// component document has to be, never the parser's diagnostic text, so the message one
/// malformed document earns does not depend on which reader read it. The document is
/// bounded before it is parsed: the modelled `data` member is `len 1..16000`, which the
/// constraint walk refuses past before a handler runs, and an aliased document is refused
/// here rather than expanded, so what the parser is handed is both small and finite.
pub fn parse_component_document(data: &str) -> Result<ComponentDocument, String> {
    // Before the loader — or the event scan that looks for an alias — sees the document
    // at all: how deep it may make either of them recurse.
    if guard_document_nesting(data).is_none() {
        return Err("The component document nests too deeply.".to_string());
    }
    if document_uses_aliases(data) {
        return Err("The component document must not use YAML aliases.".to_string());
    }
    let mut documents = ::yaml_rust2::YamlLoader::load_from_str(data)
        .map_err(|_| "The component document is not a valid YAML document.".to_string())?;
    if documents.len() != 1 {
        return Err(format!(
            "The component document must be a single YAML document; this request carries {}.",
            documents.len()
        ));
    }
    let document = documents.remove(0);
    let Some(map) = document.as_hash() else {
        return Err(
            "The component document must be a YAML mapping of document keys to values.".to_string(),
        );
    };

    let phases = mapping_get(map, PHASES_KEY)
        .and_then(::yaml_rust2::Yaml::as_vec)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let only_test_phase = phases.len() == 1
        && phases.iter().all(|phase| {
            phase
                .as_hash()
                .and_then(|phase| mapping_get(phase, PHASE_NAME_KEY))
                .and_then(scalar_string)
                .as_deref()
                == Some(TEST_PHASE_NAME)
        });

    Ok(ComponentDocument {
        component_type: if only_test_phase {
            COMPONENT_TYPE_TEST.to_string()
        } else {
            COMPONENT_TYPE_BUILD.to_string()
        },
        parameters: declared_parameters(map),
    })
}

/// The parameters a component document declares, each as the modelled
/// `ComponentParameterDetail` a read reports.
///
/// A component document lists its parameters as single-key mappings of the parameter's
/// name to its details, so a list entry that is not one declares no parameter and is
/// passed over. `type` is the one detail the modelled structure requires, so an entry
/// without it declares nothing a read could report.
fn declared_parameters(document: &::yaml_rust2::yaml::Hash) -> Option<Vec<ComponentParameterDetail>> {
    let declared = mapping_get(document, PARAMETERS_KEY)?.as_vec()?;
    let parameters: Vec<ComponentParameterDetail> = declared
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_hash()?;
            let (name, details) = entry.iter().next()?;
            let name = scalar_string(name)?;
            let details = details.as_hash()?;
            let parameter_type = mapping_get(details, PARAMETER_TYPE_KEY).and_then(scalar_string)?;
            Some(ComponentParameterDetail {
                name,
                r#type: parameter_type,
                default_value: mapping_get(details, PARAMETER_DEFAULT_KEY).and_then(scalar_values),
                description: mapping_get(details, PARAMETER_DESCRIPTION_KEY).and_then(scalar_string),
            })
        })
        .collect();
    (!parameters.is_empty()).then_some(parameters)
}

/// The value `key` carries in `map`, if any.
fn mapping_get<'document>(
    map: &'document ::yaml_rust2::yaml::Hash,
    key: &str,
) -> Option<&'document ::yaml_rust2::Yaml> {
    map.get(&::yaml_rust2::Yaml::String(key.to_string()))
        .or_else(|| {
            map.iter()
                .find(|(candidate, _)| candidate.as_str() == Some(key))
                .map(|(_, value)| value)
        })
}

/// One scalar node as the text a modelled string member carries.
fn scalar_string(value: &::yaml_rust2::Yaml) -> Option<String> {
    match value {
        ::yaml_rust2::Yaml::String(text) | ::yaml_rust2::Yaml::Real(text) => Some(text.clone()),
        ::yaml_rust2::Yaml::Integer(number) => Some(number.to_string()),
        ::yaml_rust2::Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// One node as the list of values a modelled string-list member carries: a scalar is the
/// one-value list, a sequence is its scalars.
fn scalar_values(value: &::yaml_rust2::Yaml) -> Option<Vec<String>> {
    match value {
        ::yaml_rust2::Yaml::Array(items) => {
            Some(items.iter().filter_map(scalar_string).collect::<Vec<_>>())
        }
        scalar => scalar_string(scalar).map(|value| vec![value]),
    }
    .filter(|values| !values.is_empty())
}

/// Whether `data` carries a YAML alias.
///
/// An alias is read from the event stream rather than from a loaded tree, so the
/// document is never expanded to find one: an aliased document is refused, so no
/// caller-supplied document can expand into more memory than it occupies on the wire.
fn document_uses_aliases(data: &str) -> bool {
    #[derive(Default)]
    struct AliasDetector {
        found: bool,
    }

    impl ::yaml_rust2::parser::EventReceiver for AliasDetector {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            if matches!(event, ::yaml_rust2::parser::Event::Alias(_)) {
                self.found = true;
            }
        }
    }

    let mut detector = AliasDetector::default();
    // A parse failure is not an alias; the load below reports why the document is not
    // readable.
    let _ = ::yaml_rust2::parser::Parser::new_from_str(data).load(&mut detector, true);
    detector.found
}

/// How deeply `data` can make a YAML reader recurse, refused before any reader runs.
///
/// Three measures, each a bound on NESTING rather than a count of how big the document
/// is: [`MAX_DOCUMENT_FLOW_DEPTH`] flow collections open at once with
/// [`MAX_DOCUMENT_FLOW_OPENERS`] as the backstop a byte scan needs (a `]` inside a quoted
/// scalar lowers the running depth while the loader's own depth climbs, and no document
/// nests deeper than its opener COUNT), plus [`MAX_DOCUMENT_INDENT`] columns of leading
/// indentation and [`MAX_DOCUMENT_COMPACT_PER_LINE`] compact `- `/`? ` indicators on one
/// line — the two ways block nesting descends. `None` once a document is past one of them.
fn guard_document_nesting(data: &str) -> Option<()> {
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';

    for byte in data.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_DOCUMENT_INDENT {
                    return None;
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth += 1;
                flow_openers += 1;
                if flow_depth > MAX_DOCUMENT_FLOW_DEPTH
                    || flow_openers > MAX_DOCUMENT_FLOW_OPENERS
                {
                    return None;
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the
        // beginning of a line or after a space — and only when a space follows it, which
        // is what separates `- step` from the hyphen in `my-component` or `a-b-c`.
        if matches!(previous, b'-' | b'?') && matches!(byte, b' ' | b'\t' | b'\n' | b'\r') {
            compact_on_line += 1;
            if compact_on_line > MAX_DOCUMENT_COMPACT_PER_LINE {
                return None;
            }
        }
        previous = byte;
    }
    Some(())
}

// ---------------------------------------------------------------------------
// Page tokens
//
// `ListImageRecipes` is `@paginated`, and the modelled
// `InvalidPaginationTokenException` is how a token this service did not issue is
// refused. A token therefore CARRIES the page it continues — which listing, in which
// Region, under which query, and the ARN the previous page ended at — together with a
// digest over that payload, so a fabricated or edited token is refused instead of being
// read as a page position. Nothing is stored for it: listing is a read, so it writes
// nothing a later caller could observe.
// ---------------------------------------------------------------------------

/// What one page token continues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    /// The list operation that issued it ([`LIST_IMAGE_RECIPES_OPERATION`]).
    pub operation: String,
    /// The Region whose endpoint issued it.
    pub region: String,
    /// The query the page belongs to, so a token cannot be spent on a different one
    /// ([`image_recipe_query_fingerprint`]).
    pub query: String,
    /// The key the previous page ended at; the next page starts after it.
    pub after: String,
}

/// Separates the payload's fields. A unit separator, which no ARN, Region or rendered
/// query carries.
const PAGE_TOKEN_FIELD_SEPARATOR: char = '\u{1f}';

/// Separates the digest from the payload it covers.
const PAGE_TOKEN_DIGEST_SEPARATOR: char = '.';

/// How many bytes of the payload's digest the token carries.
const PAGE_TOKEN_DIGEST_BYTES: usize = 16;

/// The token that continues `cursor`.
#[must_use]
pub fn encode_page_token(cursor: &PageCursor) -> String {
    let payload = page_token_payload(cursor);
    format!(
        "{}{PAGE_TOKEN_DIGEST_SEPARATOR}{}",
        page_token_digest(&payload),
        ::hex::encode(payload.as_bytes())
    )
}

/// Where the page `token` continues starts, or `None` when `token` is not a token this
/// service issued for this listing.
#[must_use]
pub fn decode_page_token(token: &str, cursor_of: &PageCursor) -> Option<String> {
    let (digest, payload) = token.split_once(PAGE_TOKEN_DIGEST_SEPARATOR)?;
    let payload = String::from_utf8(::hex::decode(payload).ok()?).ok()?;
    if page_token_digest(&payload) != digest {
        return None;
    }
    let mut fields = payload.split(PAGE_TOKEN_FIELD_SEPARATOR);
    let operation = fields.next()?;
    let region = fields.next()?;
    let query = fields.next()?;
    let after = fields.next()?;
    if fields.next().is_some()
        || operation != cursor_of.operation
        || region != cursor_of.region
        || query != cursor_of.query
    {
        return None;
    }
    Some(after.to_string())
}

/// The payload a token carries, field by field.
fn page_token_payload(cursor: &PageCursor) -> String {
    [
        cursor.operation.as_str(),
        cursor.region.as_str(),
        cursor.query.as_str(),
        cursor.after.as_str(),
    ]
    .join(&PAGE_TOKEN_FIELD_SEPARATOR.to_string())
}

/// The digest a token carries over its payload, as hex.
fn page_token_digest(payload: &str) -> String {
    use ::sha2::Digest;

    let digest = ::sha2::Sha256::digest(payload.as_bytes());
    ::hex::encode(&digest[..PAGE_TOKEN_DIGEST_BYTES])
}

/// A deterministic rendering of which image recipes one `ListImageRecipes` request asked
/// for, so a token issued for one query is refused for another.
///
/// `maxResults` is deliberately NOT part of it: the page size describes how much of the
/// listing to answer with, not which listing it is, so a caller may change it between
/// pages.
#[must_use]
pub fn image_recipe_query_fingerprint(owner: Option<&str>, filters: Option<&[Filter]>) -> String {
    let rendered_filters = filters.map_or_else(
        || ABSENT.to_string(),
        |filters| {
            group(
                &filters
                    .iter()
                    .map(|filter| {
                        group(&[
                            text(filter.name.as_deref()),
                            list(filter.values.as_deref()),
                        ])
                    })
                    .collect::<Vec<_>>(),
            )
        },
    );
    format!(
        "owner={};filters={rendered_filters};",
        text(Some(owner.unwrap_or(OWNERSHIP_SELF)))
    )
}

/// The region this request was signed for, from its SigV4 credential scope; the
/// emulator's default region when it carries none.
///
/// The scope is caller-controlled text and the value lands inside an ARN whose
/// modelled pattern allows only a non-colon field there, so anything carrying a colon
/// or anything implausibly long is refused here and the default is used instead: a
/// generated ARN can never carry text the model does not admit. A request whose scope
/// is unusable is therefore served by the DEFAULT Region, so the configuration it
/// creates lives there and is read back through that Region's ARN — the same place a
/// request carrying no scope at all is served.
#[must_use]
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Render `at` the way this model's string `DateTime` members carry an instant
/// (ISO 8601, e.g. `2026-10-24T01:51:48Z`).
#[must_use]
pub fn iso8601(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// Mint the `requestId` one response names the request it answers.
///
/// 8-4-4-4-12 lowercase hex, the form an AWS request id takes. The VALUE is per-call
/// and is not a requirement: nothing can predict it and no caller may depend on a
/// particular one. What it must do is name ONE response, so it is drawn from a
/// per-process counter run through [`scramble`] — a bijection, so two responses never
/// carry the same id — mixed with the instant the response was answered, so two runs
/// of this emulator do not mint the same sequence.
#[must_use]
pub fn new_request_id() -> String {
    static MINTED: AtomicU64 = AtomicU64::new(0);

    let counter = MINTED.fetch_add(1, Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let instant = since_epoch
        .as_secs()
        .wrapping_mul(1_000_000_000)
        .wrapping_add(u64::from(since_epoch.subsec_nanos()));
    // The high half is a bijection of the counter alone, which is what makes two ids
    // minted in the same nanosecond differ; the low half carries the instant.
    let high = scramble(counter);
    let low = scramble(instant ^ scramble(counter.wrapping_add(1)));
    let hex = format!("{high:016x}{low:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Mix a counter so consecutive values render as unrelated tokens.
///
/// SplitMix64's finaliser applied to the counter times the 64-bit golden ratio. Every
/// step (multiply by an odd constant, xor-shift) is a bijection on `u64`, so the whole
/// function is one: distinct counters never collide.
fn scramble(seq: u64) -> u64 {
    let mut z = seq.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A caller-submitted tag map, ordered so a read reports it the same way twice.
#[must_use]
pub fn into_ordered_tags(tags: HashMap<String, String>) -> BTreeMap<String, String> {
    tags.into_iter().collect()
}

/// An ordered tag map as the modelled map member carries it.
fn into_tag_map(tags: BTreeMap<String, String>) -> HashMap<String, String> {
    tags.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Idempotency
//
// `clientToken` is an @idempotencyToken: "If you retry a request with the same client
// token, Image Builder returns the original response without running the operation
// again", and a token reused with different parameter values is the modelled
// `IdempotentParameterMismatchException`. Telling those two apart needs the ORIGINAL
// request's creation parameters, so a successful create files their rendering and a
// later request carrying the same token is rendered the same way and compared.
//
// The rendering is a total, deterministic text over every creation parameter: each
// member is length-prefixed so no two different inputs can render alike, and every map
// is written in key order so one request cannot render two ways.
// ---------------------------------------------------------------------------

/// How an absent member renders inside a fingerprint.
const ABSENT: &str = "-";

/// A deterministic rendering of the creation parameters one
/// `CreateInfrastructureConfiguration` request carries.
///
/// `clientToken` and `dryRun` are deliberately NOT part of it: the token is the key
/// this rendering is compared under, and a dry run never files a receipt.
#[must_use]
pub fn infrastructure_configuration_request_fingerprint(
    input: &CreateInfrastructureConfigurationInput,
) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "instanceProfileName",
        text(Some(input.instance_profile_name.as_str())),
    );
    fields.insert("subnetId", text(input.subnet_id.as_deref()));
    fields.insert("keyPair", text(input.key_pair.as_deref()));
    fields.insert("snsTopicArn", text(input.sns_topic_arn.as_deref()));
    fields.insert("instanceTypes", list(input.instance_types.as_deref()));
    fields.insert("securityGroupIds", list(input.security_group_ids.as_deref()));
    fields.insert("resourceTags", tag_map(input.resource_tags.as_ref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert(
        "terminateInstanceOnFailure",
        flag(input.terminate_instance_on_failure),
    );
    fields.insert(
        "terminateInstanceOnCompletion",
        flag(input.terminate_instance_on_completion),
    );
    fields.insert(
        "logging",
        input.logging.as_ref().map_or_else(
            || ABSENT.to_string(),
            |logging| {
                logging.s3_logs.as_ref().map_or_else(
                    || ABSENT.to_string(),
                    |s3| {
                        group(&[
                            text(s3.s3_bucket_name.as_deref()),
                            text(s3.s3_key_prefix.as_deref()),
                        ])
                    },
                )
            },
        ),
    );
    fields.insert(
        "instanceMetadataOptions",
        input.instance_metadata_options.as_ref().map_or_else(
            || ABSENT.to_string(),
            |options| {
                group(&[
                    text(options.http_tokens.as_deref()),
                    number(options.http_put_response_hop_limit),
                ])
            },
        ),
    );
    fields.insert(
        "placement",
        input.placement.as_ref().map_or_else(
            || ABSENT.to_string(),
            |placement| {
                group(&[
                    text(placement.availability_zone.as_deref()),
                    text(placement.tenancy.as_deref()),
                    text(placement.host_id.as_deref()),
                    text(placement.host_resource_group_arn.as_deref()),
                ])
            },
        ),
    );
    fields.insert(
        "parentInstance",
        input.parent_instance.as_ref().map_or_else(
            || ABSENT.to_string(),
            |parent| {
                group(&[
                    text(Some(parent.auto_scaling_group.name.as_str())),
                    number(parent.auto_scaling_group.capacity_wait_timeout_seconds),
                ])
            },
        ),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateComponent` request
/// carries. See [`infrastructure_configuration_request_fingerprint`] for what it leaves
/// out and why.
#[must_use]
pub fn component_request_fingerprint(input: &CreateComponentInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("semanticVersion", text(Some(input.semantic_version.as_str())));
    fields.insert("platform", text(Some(input.platform.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert(
        "changeDescription",
        text(input.change_description.as_deref()),
    );
    fields.insert(
        "supportedOsVersions",
        list(input.supported_os_versions.as_deref()),
    );
    fields.insert("data", text(input.data.as_deref()));
    fields.insert("uri", text(input.uri.as_deref()));
    fields.insert("kmsKeyId", text(input.kms_key_id.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// A deterministic rendering of the creation parameters one `CreateImageRecipe` request
/// carries. See [`infrastructure_configuration_request_fingerprint`] for what it leaves
/// out and why.
#[must_use]
pub fn image_recipe_request_fingerprint(input: &CreateImageRecipeInput) -> String {
    let mut fields: BTreeMap<&'static str, String> = BTreeMap::new();
    fields.insert("name", text(Some(input.name.as_str())));
    fields.insert("semanticVersion", text(Some(input.semantic_version.as_str())));
    fields.insert("parentImage", text(Some(input.parent_image.as_str())));
    fields.insert("description", text(input.description.as_deref()));
    fields.insert("workingDirectory", text(input.working_directory.as_deref()));
    fields.insert("amiWatermarks", list(input.ami_watermarks.as_deref()));
    fields.insert("tags", tag_map(input.tags.as_ref()));
    fields.insert("amiTags", tag_map(input.ami_tags.as_ref()));
    fields.insert(
        "components",
        input.components.as_ref().map_or_else(
            || ABSENT.to_string(),
            |components| {
                group(
                    &components
                        .iter()
                        .map(|configuration| {
                            group(&[
                                text(Some(configuration.component_arn.as_str())),
                                configuration.parameters.as_ref().map_or_else(
                                    || ABSENT.to_string(),
                                    |parameters| {
                                        group(
                                            &parameters
                                                .iter()
                                                .map(|parameter| {
                                                    group(&[
                                                        text(Some(parameter.name.as_str())),
                                                        list(Some(parameter.value.as_slice())),
                                                    ])
                                                })
                                                .collect::<Vec<_>>(),
                                        )
                                    },
                                ),
                            ])
                        })
                        .collect::<Vec<_>>(),
                )
            },
        ),
    );
    fields.insert(
        "blockDeviceMappings",
        input.block_device_mappings.as_ref().map_or_else(
            || ABSENT.to_string(),
            |mappings| {
                group(
                    &mappings
                        .iter()
                        .map(|mapping| {
                            group(&[
                                text(mapping.device_name.as_deref()),
                                text(mapping.virtual_name.as_deref()),
                                text(mapping.no_device.as_deref()),
                                mapping.ebs.as_ref().map_or_else(
                                    || ABSENT.to_string(),
                                    |ebs| {
                                        group(&[
                                            flag(ebs.encrypted),
                                            flag(ebs.delete_on_termination),
                                            number(ebs.iops),
                                            number(ebs.volume_size),
                                            number(ebs.throughput),
                                            text(ebs.kms_key_id.as_deref()),
                                            text(ebs.snapshot_id.as_deref()),
                                            text(ebs.volume_type.as_deref()),
                                        ])
                                    },
                                ),
                            ])
                        })
                        .collect::<Vec<_>>(),
                )
            },
        ),
    );
    fields.insert(
        "additionalInstanceConfiguration",
        input.additional_instance_configuration.as_ref().map_or_else(
            || ABSENT.to_string(),
            |configuration| {
                group(&[
                    text(configuration.user_data_override.as_deref()),
                    configuration.systems_manager_agent.as_ref().map_or_else(
                        || ABSENT.to_string(),
                        |agent| flag(agent.uninstall_after_build),
                    ),
                ])
            },
        ),
    );
    fields
        .iter()
        .map(|(member, rendered)| format!("{member}={rendered};"))
        .collect()
}

/// One string member, length-prefixed so no two different values render alike.
fn text(value: Option<&str>) -> String {
    value.map_or_else(
        || ABSENT.to_string(),
        |value| format!("{}:{value}", value.len()),
    )
}

/// One optional boolean member.
fn flag(value: Option<bool>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One optional integer member.
fn number(value: Option<i32>) -> String {
    value.map_or_else(|| ABSENT.to_string(), |value| value.to_string())
}

/// One list member, in the order the request carried it — a list's order is part of
/// its value, so two orders are two different requests.
fn list(values: Option<&[String]>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            group(
                &values
                    .iter()
                    .map(|value| text(Some(value)))
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// One map member, in key order — a map carries no order of its own, so the rendering
/// imposes one and two orderings of the same pairs render alike.
fn tag_map(values: Option<&HashMap<String, String>>) -> String {
    values.map_or_else(
        || ABSENT.to_string(),
        |values| {
            let ordered: BTreeMap<&String, &String> = values.iter().collect();
            group(
                &ordered
                    .into_iter()
                    .map(|(key, value)| {
                        format!("{}={}", text(Some(key)), text(Some(value)))
                    })
                    .collect::<Vec<_>>(),
            )
        },
    )
}

/// Several already-rendered parts as one member's value.
fn group(parts: &[String]) -> String {
    format!("[{}]", parts.join(","))
}

// ---------------------------------------------------------------------------
// How this service words a refusal
//
// Each wording lives here once, so two operations state the same failure the same way.
// ---------------------------------------------------------------------------

/// Why a request naming a subnet but no security group is refused.
///
/// The rule is the `subnetId` member's own: "If you specify `subnetId`, you must also
/// specify one or more security group IDs in `securityGroupIds`. Otherwise, the
/// request fails." The wording names the two members and the requirement, and
/// withholds the values the request carried.
#[must_use]
pub fn subnet_without_security_group_message() -> String {
    "The request must specify one or more securityGroupIds when it specifies a subnetId."
        .to_string()
}

/// Why a request assigning a reserved metadata-tag key is refused.
#[must_use]
pub fn reserved_resource_tag_key_message(key: &str) -> String {
    format!("The resourceTags key {key} is reserved and can't be assigned.")
}

/// Why a create whose name is already taken in this Region is refused.
#[must_use]
pub fn infrastructure_configuration_exists_message(name: &str) -> String {
    format!(
        "An infrastructure configuration named {name} already exists in this Amazon Web Services \
         Region."
    )
}

/// Why a read naming no stored configuration reports nothing.
///
/// Names the RESOURCE TYPE that is absent and the ARN the caller asked about, and
/// nothing about any configuration that does exist.
#[must_use]
pub fn infrastructure_configuration_not_found_message(arn: &str) -> String {
    format!("The infrastructure configuration {arn} does not exist.")
}

/// Why a create was not performed because the request asked only for validation.
///
/// From the `dryRun` member's own documentation: "Validates the required permissions
/// and request parameters without performing the operation. If validation succeeds,
/// the operation returns a DryRunOperationException error response."
#[must_use]
pub fn dry_run_message() -> String {
    "Request would have succeeded, but DryRun flag is set.".to_string()
}

/// Why the same `clientToken` carrying different creation parameters is refused.
#[must_use]
pub fn idempotent_parameter_mismatch_message() -> String {
    "The clientToken was already used for a request with different parameter values.".to_string()
}

/// Why a create is refused once the configuration store is full.
#[must_use]
pub fn infrastructure_configuration_store_full_message() -> String {
    format!(
        "This account already holds the maximum of {MAX_INFRASTRUCTURE_CONFIGURATIONS} \
         infrastructure configurations."
    )
}

/// Why a create is refused once the component store is full.
#[must_use]
pub fn component_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_COMPONENTS} components.")
}

/// Why a create is refused once the image-recipe store is full.
#[must_use]
pub fn image_recipe_store_full_message() -> String {
    format!("This account already holds the maximum of {MAX_IMAGE_RECIPES} image recipes.")
}

/// Why an operation could not reach this service's store at all.
#[must_use]
pub fn store_unavailable_message() -> String {
    "The Image Builder resource store is unavailable.".to_string()
}

/// Why a create naming a version node out of bounds is refused.
///
/// The rule is `semanticVersion`'s own: the three assignable nodes take "any positive
/// integer value, including zero. The upper limit is 2^30-1, or 1073741823, for each
/// node", and the fourth node is Image Builder's to assign. The wording names the member,
/// the version the request carried and the bound it has to satisfy.
#[must_use]
pub fn invalid_version_number_message(semantic_version: &str) -> String {
    format!(
        "The semanticVersion {semantic_version} is not valid. Each of the three assignable nodes \
         must be an integer from 0 to {MAX_VERSION_NODE}."
    )
}

/// Why a `CreateComponent` naming neither a document nor a URI, or both, is refused.
///
/// The rule is the operation's own: the component "is based on a YAML document that you
/// specify using exactly one of the following methods: Inline, using the data property in
/// the request body. A URL that points to a YAML document file stored in Amazon S3, using
/// the uri property in the request body", and `data` adds "you cannot specify both
/// properties".
#[must_use]
pub fn component_data_or_uri_message() -> String {
    "The request must specify exactly one of data and uri.".to_string()
}

/// Why a `CreateComponent` naming a URI that is not an S3 URL is refused.
///
/// From the `uri` member's own documentation: "This must be an S3 URL (s3://bucket/key)".
#[must_use]
pub fn component_uri_message(uri: &str) -> String {
    format!("The uri {uri} is not valid. A component document uri must be an S3 URL (s3://bucket/key).")
}

/// Why a `CreateComponent` repeating the latest build version's content is refused.
///
/// The rule is `CreateComponent`'s `name` member: a repeat of a name+version pair "creates
/// a new build version for it. If the content is also identical to the latest build
/// version, the request fails because the component already exists." `CreateComponent` is
/// the one create in this model that declares no `ResourceAlreadyExistsException` — its
/// sibling `CreateImageRecipe` does — so the already-exists condition of a component is
/// reported under the resource-state error it DOES declare,
/// `ResourceInUseException` (HTTP 400), with the message naming the build version that
/// already holds this content.
#[must_use]
pub fn component_already_exists_message(arn: &str) -> String {
    format!("The component {arn} already exists with identical content.")
}

/// Why a read naming no stored component reports nothing.
///
/// Names the RESOURCE TYPE that is absent and the ARN the caller asked about, and nothing
/// about any component that does exist.
#[must_use]
pub fn component_not_found_message(arn: &str) -> String {
    format!("The component {arn} does not exist.")
}

/// Why a delete of a component a stored recipe still includes is refused.
#[must_use]
pub fn component_in_use_message(arn: &str, recipe_arn: &str) -> String {
    format!("The component {arn} can't be deleted because the image recipe {recipe_arn} includes it.")
}

/// Why a create whose component list names something that is not a stored component is
/// refused.
#[must_use]
pub fn unknown_component_reference_message(reference: &str) -> String {
    format!("The componentArn {reference} does not name a component in this Amazon Web Services Region.")
}

/// Why a create whose components do not agree on one platform is refused.
///
/// A recipe builds for ONE platform, which is the platform of the components it applies,
/// so a list spanning two of them names no platform the recipe could report.
#[must_use]
pub fn mixed_component_platforms_message(first: &str, second: &str) -> String {
    format!(
        "The components of an image recipe must all target one platform; this request names both \
         {first} and {second}."
    )
}

/// Why a create naming more components than one recipe may include is refused.
///
/// The bound is the quota the modelled `components` member documents: "A recipe can
/// contain a maximum of 20 build and test components in any combination, by default. This
/// maximum is an adjustable quota."
#[must_use]
pub fn too_many_recipe_components_message(submitted: usize) -> String {
    format!(
        "An image recipe can contain a maximum of {MAX_RECIPE_COMPONENTS} components; this request \
         names {submitted}."
    )
}

/// Why a create naming one component twice is refused.
///
/// The rule is `ComponentConfiguration`'s own: "You can specify each component only once
/// in a recipe, regardless of version."
#[must_use]
pub fn duplicate_recipe_component_message(name: &str) -> String {
    format!(
        "The component {name} is named more than once; an image recipe can specify each component \
         only once, regardless of version."
    )
}

/// Why a create whose name and version are already taken in this Region is refused.
#[must_use]
pub fn image_recipe_exists_message(name: &str, semantic_version: &str) -> String {
    format!(
        "An image recipe named {name} at version {semantic_version} already exists in this Amazon \
         Web Services Region."
    )
}

/// Why a read naming no stored image recipe reports nothing.
#[must_use]
pub fn image_recipe_not_found_message(arn: &str) -> String {
    format!("The image recipe {arn} does not exist.")
}

/// Why a tagging call naming no stored resource reports nothing.
///
/// Names the ARN the caller asked about and nothing about any resource that does exist.
/// The ARN a tagging call carries may name any of the resource kinds the modelled
/// `ImageBuilderArn` admits, so the wording names "the resource" rather than one kind.
#[must_use]
pub fn tagged_resource_not_found_message(arn: &str) -> String {
    format!("The resource {arn} does not exist.")
}

/// Why a tagging call that would leave a resource holding more tags than one resource may
/// carry is refused.
///
/// The bound is the modelled `TagMap`'s own ([`MAX_RESOURCE_TAGS`]); the wording names it
/// and how many tags the merged map would have held.
#[must_use]
pub fn too_many_resource_tags_message(arn: &str, merged: usize) -> String {
    format!(
        "The resource {arn} can carry a maximum of {MAX_RESOURCE_TAGS} tags; this request would \
         leave it with {merged}."
    )
}

/// Why a list naming an unsupported filter is refused.
///
/// The supported names are the `filters` member's own list
/// ([`IMAGE_RECIPE_FILTER_NAMES`]).
#[must_use]
pub fn unsupported_filter_message(name: &str) -> String {
    format!(
        "The filter name {name} is not supported. The supported filter names are {}.",
        IMAGE_RECIPE_FILTER_NAMES.join(", ")
    )
}

/// Why a list naming a filter with no name or no values is refused.
#[must_use]
pub fn incomplete_filter_message() -> String {
    "Each filter must specify a name and one or more values.".to_string()
}

/// Why a list carrying a token this service did not issue for it is refused.
///
/// The modelled `InvalidPaginationTokenException` documents the condition itself: "You
/// have provided an invalid pagination token in your request."
#[must_use]
pub fn invalid_pagination_token_message() -> String {
    "You have provided an invalid pagination token in your request.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_differing_only_in_case_spaces_or_underscores_normalize_alike() {
        let cases: &[(&str, &str)] = &[
            ("Build-Infra", "build-infra"),
            ("build infra", "build-infra"),
            ("build_infra", "build-infra"),
            ("BUILD INFRA", "build-infra"),
            ("build-infra", "build-infra"),
            ("Prod_Linux Build-01", "prod-linux-build-01"),
        ];
        for (name, expected) in cases {
            let actual = normalized_name(name);
            assert_eq!(
                &actual, expected,
                "case {name:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_minted_request_id_is_uuid_shaped_and_fresh() {
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..16 {
            let id = new_request_id();
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "case new_request_id: expected 8-4-4-4-12 groups, got {id:?}"
            );
            assert!(
                id.chars().all(|c| c == '-' || c.is_ascii_hexdigit()),
                "case new_request_id: expected lowercase hex, got {id:?}"
            );
            assert!(
                !seen.contains(&id),
                "case new_request_id: expected a fresh id, got the already-minted {id:?}"
            );
            seen.push(id);
        }
    }

    #[test]
    fn a_semantic_version_is_read_only_inside_the_documented_bound() {
        // Each row is a version and the three nodes it assigns, or `None` for one the
        // documented upper limit (2^30-1) or the three-node syntax refuses.
        let cases: &[(&str, Option<[u64; ASSIGNED_VERSION_NODES]>)] = &[
            ("1.0.0", Some([1, 0, 0])),
            ("0.0.0", Some([0, 0, 0])),
            ("2021.1.1", Some([2021, 1, 1])),
            ("1073741823.1073741823.1073741823", Some([
                MAX_VERSION_NODE,
                MAX_VERSION_NODE,
                MAX_VERSION_NODE,
            ])),
            ("1073741824.0.0", None),
            ("1.0", None),
            ("1.0.0.0", None),
            ("1.x.x", None),
        ];
        for (version, expected) in cases {
            let actual = parse_semantic_version(version);
            assert_eq!(
                &actual, expected,
                "case {version:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn a_component_reference_reads_build_and_wildcard_version_arns() {
        let build = component_build_version_arn("us-west-2", "Demo Component", "1.2.3", 4);
        let reference = ComponentReference::parse(&build)
            .unwrap_or_else(|| panic!("case {build:?}: expected a component reference"));
        assert_eq!(
            reference,
            ComponentReference {
                region: "us-west-2".to_string(),
                name: "demo-component".to_string(),
                version: [
                    VersionNode::Exact(1),
                    VersionNode::Exact(2),
                    VersionNode::Exact(3)
                ],
                build_version: Some(4),
            },
            "case {build:?}: expected the ARN's own fields, got {reference:?}"
        );

        // A wildcard is admitted in TRAILING positions only, and a build version ARN
        // names an exact version.
        let cases: &[(&str, bool)] = &[
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.2.3", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.2.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/x.x.x", true),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/x.2.3", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.3", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:component/demo/1.x.x/1", false),
            ("arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/demo/1.2.3", false),
            ("arn:aws:ec2:us-west-2:000000000000:component/demo/1.2.3", false),
            ("not-an-arn", false),
        ];
        for (reference, readable) in cases {
            let actual = ComponentReference::parse(reference).is_some();
            assert_eq!(
                actual, *readable,
                "case {reference:?}: expected readable={readable}, got {actual}"
            );
        }
    }

    #[test]
    fn a_component_document_declares_its_type_and_parameters() {
        // One phase named `test` is the only document that is a TEST component; the rows
        // vary the phase count, the phase name and the order.
        let cases: &[(&str, &str)] = &[
            ("name: t\nphases:\n  - name: test\n    steps: []\n", COMPONENT_TYPE_TEST),
            ("name: b\nphases:\n  - name: build\n    steps: []\n", COMPONENT_TYPE_BUILD),
            (
                "name: bt\nphases:\n  - name: build\n    steps: []\n  - name: test\n    steps: []\n",
                COMPONENT_TYPE_BUILD,
            ),
            ("name: none\nschemaVersion: 1.0\n", COMPONENT_TYPE_BUILD),
        ];
        for (data, expected) in cases {
            let document = parse_component_document(data)
                .unwrap_or_else(|why| panic!("case {data:?}: expected a document, got {why}"));
            assert_eq!(
                &document.component_type, expected,
                "case {data:?}: expected type {expected:?}, got {:?}",
                document.component_type
            );
        }

        let declared = parse_component_document(
            "name: p\nparameters:\n  - Size:\n      type: string\n      default: small\n      \
             description: how big\n  - Hosts:\n      type: stringList\n      default:\n        \
             - a\n        - b\n  - NoType:\n      description: declares nothing\nphases:\n  - \
             name: build\n    steps: []\n",
        )
        .unwrap_or_else(|why| panic!("case parameters: expected a document, got {why}"));
        let parameters = declared
            .parameters
            .unwrap_or_else(|| panic!("case parameters: expected the declared parameters"));
        let actual: Vec<String> = parameters
            .iter()
            .map(|parameter| {
                format!(
                    "{}:{}:{:?}:{:?}",
                    parameter.name,
                    parameter.r#type,
                    parameter.default_value,
                    parameter.description
                )
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                "Size:string:Some([\"small\"]):Some(\"how big\")".to_string(),
                "Hosts:stringList:Some([\"a\", \"b\"]):None".to_string(),
            ],
            "case parameters: expected the two typed parameters in document order, got {actual:?}"
        );
    }

    #[test]
    fn a_document_that_is_not_one_yaml_mapping_or_uses_aliases_is_refused() {
        let deep_flow = "phases: ".to_string() + &"[".repeat(8_000);
        let quoted_closers = "phases: ".to_string() + &"[']'".repeat(4_000);
        let cases: Vec<String> = vec![
            // Two documents.
            "name: a\n---\nname: b\n".to_string(),
            // Not a mapping.
            "- name: a\n".to_string(),
            // An alias, which is refused rather than expanded.
            "anchor: &a [1, 2]\nphases: *a\n".to_string(),
            // Not YAML at all.
            "name: [a\n".to_string(),
            // Nesting built to drive the loader's recursion, refused before any reader
            // runs: open flow collections, and the same with every closer quoted so a
            // byte scan's running depth stays low while the loader's climbs.
            deep_flow,
            quoted_closers,
            // Block nesting, by indentation and by compact indicators on one line.
            " ".repeat(1_000) + "name: a\n",
            "phases:\n".to_string() + &"- ".repeat(1_000) + "x\n",
        ];
        for data in cases {
            let actual = parse_component_document(&data);
            assert!(
                actual.is_err(),
                "case {:?}…: expected a refusal, got {actual:?}",
                data.chars().take(32).collect::<String>()
            );
        }

        // A document that is big and FLAT is not deep, so none of those bounds touches
        // it: 150 steps at the document's own nesting, inside the modelled 16,000-byte
        // maximum for `data`, are read.
        let flat = "name: Flat\nphases:\n  - name: build\n    steps:\n".to_string()
            + &"      - {name: s, action: ExecuteBash, inputs: {commands: [echo hi]}}\n"
                .repeat(150);
        assert!(
            flat.len() <= 16_000,
            "case a big flat document: the row must stay inside the modelled data maximum, got {}",
            flat.len()
        );
        let actual = parse_component_document(&flat);
        assert!(
            actual.is_ok(),
            "case a big flat document: expected it to be read, got {actual:?}"
        );
    }

    #[test]
    fn a_page_token_is_read_back_only_for_the_listing_that_issued_it() {
        let cursor = PageCursor {
            operation: LIST_IMAGE_RECIPES_OPERATION.to_string(),
            region: "us-west-2".to_string(),
            query: image_recipe_query_fingerprint(Some(OWNERSHIP_SELF), None),
            after: "arn:aws:imagebuilder:us-west-2:000000000000:image-recipe/demo/1.0.0"
                .to_string(),
        };
        let token = encode_page_token(&cursor);
        let actual = decode_page_token(&token, &cursor);
        assert_eq!(
            actual.as_deref(),
            Some(cursor.after.as_str()),
            "case issued token: expected {:?}, got {actual:?}",
            cursor.after
        );

        // A token read against another listing, Region or query is not this token, and
        // neither is anything the service did not issue.
        let other_query = PageCursor {
            query: image_recipe_query_fingerprint(Some("Amazon"), None),
            ..cursor.clone()
        };
        let other_region = PageCursor {
            region: "eu-west-1".to_string(),
            ..cursor.clone()
        };
        let cases: &[(&str, String)] = &[
            ("another query", token.clone()),
            ("another region", token.clone()),
            ("fabricated", "fabricated-token".to_string()),
            ("edited digest", format!("00000000000000000000000000000000.{}", token
                .split_once('.')
                .map_or("", |(_, payload)| payload))),
        ];
        for (case, candidate) in cases {
            let cursor_of = match *case {
                "another query" => &other_query,
                "another region" => &other_region,
                _ => &cursor,
            };
            let actual = decode_page_token(candidate, cursor_of);
            assert!(
                actual.is_none(),
                "case {case}: expected a refusal, got {actual:?}"
            );
        }
    }
}
