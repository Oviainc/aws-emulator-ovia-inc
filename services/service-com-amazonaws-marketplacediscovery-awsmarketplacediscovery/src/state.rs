// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-marketplacediscovery-awsmarketplacediscovery service.
//!
//! AWS Marketplace Discovery is a READ-ONLY API: every operation is `@readonly` and the
//! model declares no create/put operation, so a client has no way to put data into the
//! catalog it reads. The emulator therefore owns a seeded catalog here, built once on
//! [`Default`], and every handler computes its response by looking the requested id up in
//! it. Nothing in this module mutates after seeding.
//!
//! Besides the catalog this module owns the three things more than one operation needs:
//!
//! * locale resolution ([`resolve_requested_locale`], [`Localized`]), per the `Locale`
//!   shape's documentation;
//! * the opaque pagination cursor the four `@paginated` operations share
//!   ([`encode_page_token`], [`decode_page_token`], [`page`]);
//! * the projections from stored records onto wire shapes
//!   ([`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::listing_summary`],
//!   [`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::purchase_options`]);
//! * the listing match set `SearchListings` pages over and `SearchFacets` counts facets
//!   from ([`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::search_matches`],
//!   [`matches_every_search_filter`]);
//! * the member-`@pattern` check the framework's input walk cannot make for an
//!   unanchored expression ([`validate_search_text`]).

use std::collections::BTreeMap;

use crate::ops::types::{
    AmazonMachineImageEbsVolume, AmazonMachineImageFulfillmentOption,
    AmazonMachineImageOperatingSystem, AmazonMachineImageRecommendation,
    AmazonMachineImageSecurityGroup, ApiFulfillmentOption, AwsSupportedService, Category,
    CloudFormationFulfillmentOption, ContainerFulfillmentOption, ContainerOperatingSystem,
    DataArtifact, DataExchangeFulfillmentOption, DocumentItem,
    Ec2ImageBuilderComponentFulfillmentOption, EksAddOnFulfillmentOption,
    EksAddOnOperatingSystem, FreeTrialPricingTerm, FulfillmentOption, FulfillmentOptionSummary,
    GrantItem, HelmFulfillmentOption, RenewalTerm,
    HelmOperatingSystem, LegalTerm, ListingBadge, ListingSummary,
    ListingSummaryAssociatedEntity, OfferInformation, OfferSetInformation, OfferTerm, PricingModel,
    PricingUnit, ProductInformation, ProfessionalServicesFulfillmentOption,
    PurchaseOptionAssociatedEntity, PurchaseOptionBadge,
    PromotionalEmbeddedImage, PromotionalEmbeddedVideo, PromotionalMedia, PurchaseOptionSummary,
    RateCardItem, RecurringPaymentTerm, Resource, ReviewSourceSummary,
    ReviewSummary, SaasFulfillmentOption, SageMakerAlgorithmFulfillmentOption,
    SageMakerAlgorithmRecommendation, SageMakerModelFulfillmentOption,
    SageMakerModelRecommendation, SearchFilter, SellerEngagement, SellerInformation, SupportTerm,
    UsageBasedPricingTerm, UsageBasedRateCardItem, UseCase, UseCaseEntry, ValidityTerm,
};

/// The locale the service returns content in when none is requested, and the fallback for
/// a supported locale the catalog has no translation for.
///
/// Source: the `Locale` shape's documentation —
/// `smithy_query(query="doc",shape="Locale")`: "If omitted or null, the service returns
/// content in the default locale (`en-US`)".
pub const DEFAULT_LOCALE: &str = "en-US";

/// The locales this service supports.
///
/// Source: the `Locale` shape's documentation —
/// `smithy_query(query="doc",shape="Locale")`: `en-US` (service default, also resolves
/// from `en`), `fr`, `es`, `ko`, `ja`.
pub const SUPPORTED_LOCALES: &[&str] = &[DEFAULT_LOCALE, "fr", "es", "ko", "ja"];

/// The `VISIBILITY_SCOPE` value `ListPurchaseOptions` accepts.
///
/// Source: `PurchaseOptionFilter$filterValues` documentation — "`VISIBILITY_SCOPE` – The
/// visibility scope to filter by: `PRIVATE`".
pub const VISIBILITY_SCOPE_FILTER_VALUES: &[&str] = &["PRIVATE"];

/// The `AVAILABILITY_STATUS` values `ListPurchaseOptions` accepts.
///
/// Source: `PurchaseOptionFilter$filterValues` documentation — "`AVAILABILITY_STATUS` –
/// One or more availability statuses to filter by: `AVAILABLE` or `EXPIRED`".
pub const AVAILABILITY_STATUS_VALUES: &[&str] = &["AVAILABLE", "EXPIRED"];

/// The visibility scope of a purchase option that only invited buyers see.
pub const VISIBILITY_PRIVATE: &str = "PRIVATE";
/// The visibility scope of a purchase option every buyer sees.
pub const VISIBILITY_PUBLIC: &str = "PUBLIC";

/// Seconds in a day, for the seeded catalog's relative timestamps.
const DAY: i64 = 24 * 60 * 60;

/// The human-readable name of every listing badge, which is what `ListingBadge$displayName`
/// carries.
///
/// Source: the `ListingBadgeType` enum
/// (`smithy_query(query="shape",shape="ListingBadgeType")`), generated as
/// `LISTING_BADGE_TYPE_VALUES`. The order is the order a listing reports its badges in, so
/// two listings never disagree about where a badge appears.
pub const LISTING_BADGE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("AWS_FREE_TIER", "AWS Free Tier"),
    ("DEPLOYED_ON_AWS", "Deployed on AWS"),
    ("FREE_TRIAL", "Free trial"),
    ("MULTI_PRODUCT", "Multiple products"),
    ("QUICK_LAUNCH", "Quick Launch"),
];

/// The human-readable name of every purchase-option badge, which is what
/// `PurchaseOptionBadge$displayName` carries.
///
/// Source: the `PurchaseOptionBadgeType` enum
/// (`smithy_query(query="shape",shape="PurchaseOptionBadgeType")`), generated as
/// `PURCHASE_OPTION_BADGE_TYPE_VALUES`; the names are AWS's label for each value
/// (`AUTO_RENEW` is rendered hyphenated, "Auto-renew"). The order is the enum's, so two
/// purchase options never disagree about where a badge appears, exactly as
/// [`LISTING_BADGE_DISPLAY_NAMES`] does for a listing.
pub const PURCHASE_OPTION_BADGE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("PRIVATE_PRICING", "Private pricing"),
    ("FUTURE_DATED", "Future dated"),
    ("REPLACEMENT_OFFER", "Replacement offer"),
    ("AUTO_RENEW", "Auto-renew"),
];

/// The human-readable name of every fulfillment option type, which is what both
/// `FulfillmentOptionSummary$displayName` ("The human-readable name of the fulfillment
/// option type") and each fulfillment option's `fulfillmentOptionDisplayName` ("A
/// human-readable name for the fulfillment option type") carry — so one table serves both
/// and a product's detail can never disagree with its summary on a type's name.
///
/// Source: the `FulfillmentOptionType` enum's values
/// (`smithy_query(query="shape",shape="FulfillmentOptionType")`) paired with the
/// `FulfillmentOption` union member documentation that names each one.
pub const FULFILLMENT_OPTION_TYPE_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("AMAZON_MACHINE_IMAGE", "Amazon Machine Image"),
    ("API", "API"),
    ("CLOUDFORMATION_TEMPLATE", "AWS CloudFormation template"),
    ("CONTAINER", "Container"),
    ("HELM", "Helm chart"),
    ("EKS_ADD_ON", "Amazon EKS add-on"),
    ("EC2_IMAGE_BUILDER_COMPONENT", "EC2 Image Builder component"),
    ("DATA_EXCHANGE", "AWS Data Exchange"),
    ("PROFESSIONAL_SERVICES", "Professional services"),
    ("SAAS", "SaaS"),
    ("SAGEMAKER_ALGORITHM", "SageMaker algorithm"),
    ("SAGEMAKER_MODEL", "SageMaker model"),
];

/// The `DeployedOnAwsStatus` each `FulfillmentOptionType` implies for the product it
/// fulfills.
///
/// Source: `GetProductOutput$deployedOnAws` — "Indicates whether the product is deployed on
/// AWS infrastructure" (`smithy_query(query="shape",shape="GetProductOutput")`) — read
/// against what each `FulfillmentOption` union member delivers:
///
/// * `DEPLOYED` — the option delivers an artifact that RUNS on AWS infrastructure once
///   fulfilled: a machine image, a CloudFormation stack, a container, a Helm chart, an EKS
///   add-on, an Image Builder component, a SageMaker algorithm or model package.
/// * `NOT_DEPLOYED` — the option delivers software the buyer only ACCESSES, hosted by the
///   seller and deployed on no AWS infrastructure of the buyer's: a SaaS subscription or an
///   API.
/// * `NOT_APPLICABLE` — the option delivers no deployable software at all, so deployment
///   does not apply: a Data Exchange data set or a professional services engagement.
///
/// The table is keyed by the model's own enum values
/// (`smithy_query(query="shape",shape="FulfillmentOptionType")`), so a product's reported
/// status cannot disagree with the fulfillment it reports.
pub const FULFILLMENT_DEPLOYMENT_STATUS: &[(&str, &str)] = &[
    ("AMAZON_MACHINE_IMAGE", "DEPLOYED"),
    ("API", "NOT_DEPLOYED"),
    ("CLOUDFORMATION_TEMPLATE", "DEPLOYED"),
    ("CONTAINER", "DEPLOYED"),
    ("HELM", "DEPLOYED"),
    ("EKS_ADD_ON", "DEPLOYED"),
    ("EC2_IMAGE_BUILDER_COMPONENT", "DEPLOYED"),
    ("DATA_EXCHANGE", "NOT_APPLICABLE"),
    ("PROFESSIONAL_SERVICES", "NOT_APPLICABLE"),
    ("SAAS", "NOT_DEPLOYED"),
    ("SAGEMAKER_ALGORITHM", "DEPLOYED"),
    ("SAGEMAKER_MODEL", "DEPLOYED"),
];

/// The deployment status one fulfillment option type implies, per
/// [`FULFILLMENT_DEPLOYMENT_STATUS`].
///
/// A `FulfillmentOptionType` the model gains later is not classed yet, and a fulfillment
/// nothing is known about supports no claim that the product runs on AWS — so it reports
/// `NOT_APPLICABLE` rather than guessing a deployment.
pub fn deployment_status(option_type: &str) -> &'static str {
    FULFILLMENT_DEPLOYMENT_STATUS
        .iter()
        .find(|(fulfillment, _)| *fulfillment == option_type)
        .map_or("NOT_APPLICABLE", |(_, status)| *status)
}

/// The human-readable name of one fulfillment option type, or the type itself when the
/// model grows a value this table does not name yet.
pub fn fulfillment_option_display_name(option_type: &str) -> String {
    FULFILLMENT_OPTION_TYPE_DISPLAY_NAMES
        .iter()
        .find(|(value, _)| *value == option_type)
        .map_or_else(|| option_type.to_string(), |(_, name)| (*name).to_string())
}

/// The `fulfillmentOptionId` of a fulfillment option, whichever member of the
/// `FulfillmentOption` union it is: every member of the union declares the member.
pub fn fulfillment_option_id(option: &FulfillmentOption) -> &str {
    match option {
        FulfillmentOption::amazonMachineImageFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::apiFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::cloudFormationFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::containerFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::helmFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::eksAddOnFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::ec2ImageBuilderComponentFulfillmentOption(o) => {
            &o.fulfillment_option_id
        }
        FulfillmentOption::dataExchangeFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::professionalServicesFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::saasFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::sageMakerAlgorithmFulfillmentOption(o) => &o.fulfillment_option_id,
        FulfillmentOption::sageMakerModelFulfillmentOption(o) => &o.fulfillment_option_id,
    }
}

/// The `fulfillmentOptionType` of a fulfillment option, whichever member of the
/// `FulfillmentOption` union it is.
pub fn fulfillment_option_type(option: &FulfillmentOption) -> &str {
    match option {
        FulfillmentOption::amazonMachineImageFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::apiFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::cloudFormationFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::containerFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::helmFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::eksAddOnFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::ec2ImageBuilderComponentFulfillmentOption(o) => {
            &o.fulfillment_option_type
        }
        FulfillmentOption::dataExchangeFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::professionalServicesFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::saasFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::sageMakerAlgorithmFulfillmentOption(o) => &o.fulfillment_option_type,
        FulfillmentOption::sageMakerModelFulfillmentOption(o) => &o.fulfillment_option_type,
    }
}

/// Replace a fulfillment option's `usageInstructions` — the buyer-facing text every
/// member of the union that has any carries. Answers `false` for the two members the
/// model gives no instructions (`DataExchangeFulfillmentOption`,
/// `ProfessionalServicesFulfillmentOption`), which have nothing to translate.
fn set_usage_instructions(option: &mut FulfillmentOption, instructions: String) -> bool {
    let instructions = Some(instructions);
    match option {
        FulfillmentOption::amazonMachineImageFulfillmentOption(o) => {
            o.usage_instructions = instructions;
        }
        FulfillmentOption::apiFulfillmentOption(o) => o.usage_instructions = instructions,
        FulfillmentOption::cloudFormationFulfillmentOption(o) => {
            o.usage_instructions = instructions;
        }
        FulfillmentOption::containerFulfillmentOption(o) => o.usage_instructions = instructions,
        FulfillmentOption::helmFulfillmentOption(o) => o.usage_instructions = instructions,
        FulfillmentOption::eksAddOnFulfillmentOption(o) => o.usage_instructions = instructions,
        FulfillmentOption::ec2ImageBuilderComponentFulfillmentOption(o) => {
            o.usage_instructions = instructions;
        }
        FulfillmentOption::saasFulfillmentOption(o) => o.usage_instructions = instructions,
        FulfillmentOption::sageMakerAlgorithmFulfillmentOption(o) => {
            o.usage_instructions = instructions;
        }
        FulfillmentOption::sageMakerModelFulfillmentOption(o) => {
            o.usage_instructions = instructions;
        }
        FulfillmentOption::dataExchangeFulfillmentOption(_)
        | FulfillmentOption::professionalServicesFulfillmentOption(_) => return false,
    }
    true
}

/// A fulfillment option of a product, as the catalog stores it: the option in the default
/// locale plus whatever translations of its buyer-facing text the catalog holds.
///
/// A translation is derived FROM the default-locale option, so it can never disagree with
/// it on the option's identity, type or deployment detail.
#[derive(Debug, Clone)]
pub struct FulfillmentOptionRecord {
    default: FulfillmentOption,
    translations: BTreeMap<String, FulfillmentOption>,
}

impl FulfillmentOptionRecord {
    /// Store `option` as this product's fulfillment option in the default locale.
    pub fn new(option: FulfillmentOption) -> Self {
        Self {
            default: option,
            translations: BTreeMap::new(),
        }
    }

    /// Record `locale`'s translation of this option's usage instructions.
    ///
    /// PANICS in a debug build when this option's union member carries no
    /// `usageInstructions`, so a translation that would silently store a copy of the
    /// default-locale option is caught where it is written.
    #[must_use]
    pub fn translate(mut self, locale: &str, usage_instructions: &str) -> Self {
        let mut translated = self.default.clone();
        let applied = set_usage_instructions(&mut translated, usage_instructions.to_string());
        debug_assert!(
            applied,
            "fulfillment option {} of type {} carries no usageInstructions to translate",
            self.id(),
            self.option_type()
        );
        self.translations.insert(locale.to_string(), translated);
        self
    }

    /// This option in `locale`, falling back to the default-locale option.
    pub fn option(&self, locale: &str) -> FulfillmentOption {
        self.translations
            .get(locale)
            .unwrap_or(&self.default)
            .clone()
    }

    /// The option's `fulfillmentOptionId`, which no translation changes.
    pub fn id(&self) -> &str {
        fulfillment_option_id(&self.default)
    }

    /// The option's `fulfillmentOptionType`, which no translation changes.
    pub fn option_type(&self) -> &str {
        fulfillment_option_type(&self.default)
    }

    /// Whether this option supports Quick Launch.
    ///
    /// Only a SaaS option can: `quickLaunch` ("Specifies whether the SaaS product supports
    /// quick-launch deployment") is a member of `SaasFulfillmentOption` alone, and no other
    /// member of the `FulfillmentOption` union models quick launch at all.
    pub fn supports_quick_launch(&self) -> bool {
        matches!(
            &self.default,
            FulfillmentOption::saasFulfillmentOption(option) if option.quick_launch == "ENABLED"
        )
    }

    /// The summary of this option's type, as `FulfillmentOptionSummary`.
    pub fn summary(&self) -> FulfillmentOptionSummary {
        FulfillmentOptionSummary {
            fulfillment_option_type: self.option_type().to_string(),
            display_name: fulfillment_option_display_name(self.option_type()),
        }
    }
}

/// Wall-clock seconds since the Unix epoch.
pub fn now_secs() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    )
    .unwrap_or(0)
}

/// Translatable text of a catalog record.
#[derive(Debug, Clone, Default)]
pub struct LocalizedText {
    pub name: String,
    pub short_description: String,
    pub long_description: String,
}

/// A record's text in the default locale plus whatever translations the catalog holds.
///
/// Holding the default-locale text outside the translation map makes "there is always
/// content in the default locale" a property of the type rather than of the seed.
#[derive(Debug, Clone, Default)]
pub struct Localized {
    pub default: LocalizedText,
    pub translations: BTreeMap<String, LocalizedText>,
}

impl Localized {
    /// The locale the content of this record is actually returned in.
    ///
    /// Per the `Locale` documentation: the preferred locale when translated content for it
    /// exists, and the default locale when it does not.
    pub fn content_locale(&self, preferred: &'static str) -> &'static str {
        if preferred == DEFAULT_LOCALE || self.translations.contains_key(preferred) {
            preferred
        } else {
            DEFAULT_LOCALE
        }
    }

    /// The text for `locale`, falling back to the default-locale text.
    pub fn text(&self, locale: &str) -> &LocalizedText {
        self.translations.get(locale).unwrap_or(&self.default)
    }
}

/// A listing in the seeded catalog.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub listing_id: String,
    pub text: Localized,
    pub logo_thumbnail_url: String,
    pub integration_guide: Option<String>,
    pub categories: Vec<Category>,
    pub highlights: Vec<String>,
    pub pricing_models: Vec<PricingModel>,
    pub pricing_units: Vec<PricingUnit>,
    /// Products this listing is associated with; every one of them points back here
    /// through [`Product::listing_id`]. A listing's fulfillment option summaries are
    /// derived from these products' fulfillment options
    /// ([`ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState::listing_fulfillment_option_summaries`]).
    pub product_ids: Vec<String>,
    /// Offers this listing is associated with.
    pub offer_ids: Vec<String>,
    pub publisher: SellerInformation,
    pub promotional_media: Vec<PromotionalMedia>,
    pub resources: Vec<Resource>,
    pub seller_engagements: Vec<SellerEngagement>,
    pub use_cases: Vec<UseCaseEntry>,
    pub review_summary: Option<ReviewSummary>,
}

/// A product in the seeded catalog.
#[derive(Debug, Clone, Default)]
pub struct Product {
    pub product_id: String,
    /// The listing whose `associatedEntities` name this product.
    pub listing_id: String,
    pub text: Localized,
    pub manufacturer: SellerInformation,
    pub logo_thumbnail_url: String,
    pub categories: Vec<Category>,
    pub highlights: Vec<String>,
    pub promotional_media: Vec<PromotionalMedia>,
    pub resources: Vec<Resource>,
    pub seller_engagements: Vec<SellerEngagement>,
    /// How a buyer can deploy or access this product. The SINGLE record of this
    /// product's fulfillment: `ListFulfillmentOptions` pages over these options and the
    /// `fulfillmentOptionSummaries` of `GetProduct`, `GetListing` and `SearchListings`
    /// are derived from them, so the detail and the summaries cannot disagree.
    pub fulfillment_options: Vec<FulfillmentOptionRecord>,
}

impl Product {
    /// Whether this product is deployed on AWS infrastructure, which is what
    /// `GetProductOutput$deployedOnAws` reports ("Indicates whether the product is deployed
    /// on AWS infrastructure").
    ///
    /// Derived from the product's OWN fulfillment options rather than stored beside them,
    /// exactly as its `fulfillmentOptionSummaries` are: whether anything of the product runs
    /// on AWS infrastructure is decided by how the product is delivered
    /// ([`deployment_status`]), so a stored value could claim a deployment the product's
    /// fulfillment contradicts — a SaaS product the seller hosts reporting that deployment
    /// does not apply to it, say.
    ///
    /// A product fulfilled several ways reports the strongest claim its options support:
    /// `DEPLOYED` when any option deploys it onto AWS, else `NOT_DEPLOYED` when any option
    /// delivers software the buyer does not deploy, else `NOT_APPLICABLE`. A product with no
    /// fulfillment option at all deploys nothing, so deployment does not apply to it.
    pub fn deployed_on_aws(&self) -> &'static str {
        let mut deployed = false;
        let mut not_deployed = false;
        for record in &self.fulfillment_options {
            match deployment_status(record.option_type()) {
                "DEPLOYED" => deployed = true,
                "NOT_DEPLOYED" => not_deployed = true,
                _ => {}
            }
        }
        if deployed {
            "DEPLOYED"
        } else if not_deployed {
            "NOT_DEPLOYED"
        } else {
            "NOT_APPLICABLE"
        }
    }

    /// This product's fulfillment options in `locale`, ordered by `fulfillmentOptionId`
    /// so every page of `ListFulfillmentOptions` is cut from one stable total order.
    pub fn fulfillment_options(&self, locale: &str) -> Vec<FulfillmentOption> {
        let mut records: Vec<&FulfillmentOptionRecord> = self.fulfillment_options.iter().collect();
        records.sort_by(|a, b| a.id().cmp(b.id()));
        records
            .into_iter()
            .map(|record| record.option(locale))
            .collect()
    }

    /// The summary of each fulfillment option TYPE this product offers, one per distinct
    /// type in the order the options are paged over.
    ///
    /// `FulfillmentOptionSummary` summarises a type rather than an option, so two options
    /// of the same type (two AMI versions, say) summarise to one entry.
    pub fn fulfillment_option_summaries(&self) -> Vec<FulfillmentOptionSummary> {
        let mut records: Vec<&FulfillmentOptionRecord> = self.fulfillment_options.iter().collect();
        records.sort_by(|a, b| a.id().cmp(b.id()));
        let mut summaries: Vec<FulfillmentOptionSummary> = Vec::new();
        for record in records {
            if !summaries
                .iter()
                .any(|summary| summary.fulfillment_option_type == record.option_type())
            {
                summaries.push(record.summary());
            }
        }
        summaries
    }
}

/// An offer in the seeded catalog.
#[derive(Debug, Clone, Default)]
pub struct Offer {
    pub offer_id: String,
    /// The offer name in the default locale.
    pub offer_name: Option<String>,
    /// The offer name in each locale the catalog has a translation for.
    pub name_translations: BTreeMap<String, String>,
    pub expiration_time: Option<i64>,
    pub available_from_time: Option<i64>,
    pub seller_of_record: SellerInformation,
    pub product_ids: Vec<String>,
    pub offer_set_id: Option<String>,
    pub agreement_proposal_id: String,
    pub replacement_agreement_id: Option<String>,
    pub pricing_model: PricingModel,
    pub visibility_scope: String,
}

impl Offer {
    /// The locale this offer's text is returned in: the preferred one when the catalog has
    /// a translation of the offer name, the default locale otherwise.
    pub fn content_locale(&self, preferred: &'static str) -> &'static str {
        if preferred == DEFAULT_LOCALE || self.name_translations.contains_key(preferred) {
            preferred
        } else {
            DEFAULT_LOCALE
        }
    }

    /// The offer name in `locale`, falling back to the default-locale name.
    pub fn name(&self, locale: &str) -> Option<String> {
        self.name_translations
            .get(locale)
            .cloned()
            .or_else(|| self.offer_name.clone())
    }
}

/// An offer set in the seeded catalog.
#[derive(Debug, Clone, Default)]
pub struct OfferSet {
    pub offer_set_id: String,
    pub offer_set_name: Option<String>,
    pub available_from_time: Option<i64>,
    pub expiration_time: Option<i64>,
    pub buyer_notes: Option<String>,
    pub seller_of_record: SellerInformation,
    pub offer_ids: Vec<String>,
    pub visibility_scope: String,
}

/// One row of `ListPurchaseOptions`: the wire summary plus the indexed facts
/// `PurchaseOptionFilter` filters on.
#[derive(Debug, Clone)]
pub struct PurchaseOptionEntry {
    pub summary: PurchaseOptionSummary,
    pub product_ids: Vec<String>,
    pub seller_profile_id: String,
    pub visibility_scope: String,
    pub availability_status: String,
}

/// Per-service state for com-amazonaws-marketplacediscovery-awsmarketplacediscovery. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The catalog name stamped on every response that carries a `catalog` member.
    pub catalog: String,
    pub listings: BTreeMap<String, Listing>,
    pub products: BTreeMap<String, Product>,
    pub offers: BTreeMap<String, Offer>,
    pub offer_sets: BTreeMap<String, OfferSet>,
    /// The terms attached to each offer, in the order `GetOfferTerms` pages over them.
    pub offer_terms: BTreeMap<String, Vec<OfferTerm>>,
}

impl Default for ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    fn default() -> Self {
        seed_catalog()
    }
}

impl ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    /// The listings a `searchText`/`filters` pair selects, in listing-id order.
    ///
    /// This is the match set BOTH search operations answer from: `SearchListings` orders and
    /// pages it and reports its size as `totalResults`, and `SearchFacets` counts facet
    /// values over it and reports the same size — so the two can never disagree about how
    /// many listings a query matches.
    ///
    /// `searchText` is a KEYWORD query: a listing matches when at least one of the query's
    /// terms ([`search_terms`]) occurs, case-insensitively, in the text that listing presents
    /// ([`Self::search_haystack`]) — so the words of a query need not appear verbatim, next to
    /// each other or in one member for the listing to be a match. A `searchText` carrying no
    /// term at all (absent, blank, punctuation only) selects every listing. `filters` then
    /// narrow the result per [`matches_every_search_filter`]. `Err` is that function's refusal
    /// message.
    pub fn search_matches(
        &self,
        preferred: &'static str,
        search_text: &str,
        filters: &[SearchFilter],
    ) -> Result<Vec<&Listing>, String> {
        // Refuse a malformed `filters` list BEFORE any listing is considered, so a request is
        // refused for what it says rather than for whether a listing happened to reach the
        // predicate below.
        validate_search_filters(filters)?;
        let terms = search_terms(search_text);
        let mut matches: Vec<&Listing> = Vec::new();
        for listing in self.listings.values() {
            if !terms.is_empty() {
                let haystack = self.search_haystack(listing, preferred);
                if !terms.iter().any(|term| haystack.contains(term)) {
                    continue;
                }
            }
            if !matches_every_search_filter(self, listing, filters)? {
                continue;
            }
            matches.push(listing);
        }
        Ok(matches)
    }

    /// The text one `searchText` term is matched against: everything the listing PRESENTS —
    /// its name and short description, the publisher it is sold by, the categories it is
    /// classified under, the fulfillment options, pricing models and pricing units it is sold
    /// through, and the products it associates with their manufacturers — lowercased.
    ///
    /// It is read off the listing's own [`Self::listing_summary`], so the text a keyword can
    /// match is exactly the text the client is shown for that listing and the two cannot
    /// drift. IDENTIFIERS are deliberately not part of it: an id is what `filters` select a
    /// listing by, while `searchText` is the keyword query over its content.
    pub fn search_haystack(&self, listing: &Listing, preferred: &'static str) -> String {
        let summary = self.listing_summary(listing, preferred);
        let mut parts: Vec<String> = vec![
            summary.listing_name,
            summary.short_description,
            summary.publisher.display_name,
        ];
        parts.extend(
            summary
                .categories
                .into_iter()
                .map(|category| category.display_name),
        );
        parts.extend(
            summary
                .fulfillment_option_summaries
                .into_iter()
                .map(|option| option.display_name),
        );
        parts.extend(
            summary
                .pricing_models
                .into_iter()
                .map(|model| model.display_name),
        );
        parts.extend(
            summary
                .pricing_units
                .into_iter()
                .map(|unit| unit.display_name),
        );
        for entity in summary.associated_entities {
            if let Some(product) = entity.product {
                parts.push(product.product_name);
                parts.push(product.manufacturer.display_name);
            }
        }
        parts.join(" ").to_lowercase()
    }

    /// The listing summary `SearchListings` returns for `listing`, in `preferred` locale.
    pub fn listing_summary(&self, listing: &Listing, preferred: &'static str) -> ListingSummary {
        let locale = listing.text.content_locale(preferred);
        let text = listing.text.text(locale);
        ListingSummary {
            listing_id: listing.listing_id.clone(),
            listing_name: text.name.clone(),
            publisher: listing.publisher.clone(),
            fulfillment_option_summaries: self.listing_fulfillment_option_summaries(listing),
            catalog: self.catalog.clone(),
            short_description: text.short_description.clone(),
            logo_thumbnail_url: listing.logo_thumbnail_url.clone(),
            categories: listing.categories.clone(),
            badges: self.listing_badges(listing),
            review_summary: listing.review_summary.clone().unwrap_or_default(),
            pricing_models: listing.pricing_models.clone(),
            pricing_units: listing.pricing_units.clone(),
            associated_entities: listing
                .product_ids
                .iter()
                .filter_map(|id| self.product_information(id, preferred))
                .map(|product| ListingSummaryAssociatedEntity {
                    product: Some(product),
                })
                .collect(),
        }
    }

    /// The badges a listing reports: one per special attribute the listing's OWN records
    /// have, in [`LISTING_BADGE_DISPLAY_NAMES`] order.
    ///
    /// `GetListingOutput$badges` is "Badges indicating special attributes of the listing,
    /// such as free tier eligibility, free trial availability, or Quick Launch support" —
    /// each badge therefore RESTATES something the listing's products, offers and pricing
    /// already say, and is derived from them rather than stored beside them, where it could
    /// claim an attribute no record of the listing has (a Quick Launch badge on a listing
    /// whose products offer no quick-launchable fulfillment, say).
    ///
    /// What each badge type restates:
    ///
    /// * `QUICK_LAUNCH` — a product of the listing has a fulfillment option that supports
    ///   quick launch (`SaasFulfillmentOption$quickLaunch` is `ENABLED`).
    /// * `DEPLOYED_ON_AWS` — a product of the listing is `DEPLOYED` on AWS
    ///   (`Product$deployedOnAws`), which is what the `DEPLOYED_ON_AWS` search filter
    ///   matches a listing by.
    /// * `MULTI_PRODUCT` — the listing associates more than one product, the case
    ///   `GetListingOutput$integrationGuide` calls "a multi-product listing".
    /// * `FREE_TRIAL` — an offer the listing associates carries a free-trial pricing term
    ///   (`FreeTrialPricingTerm`: "enables customers to try the product before
    ///   purchasing").
    /// * `AWS_FREE_TIER` — the listing has a `FREE` pricing model, i.e. an offer a buyer
    ///   pays nothing for.
    pub fn listing_badges(&self, listing: &Listing) -> Vec<ListingBadge> {
        let products: Vec<&Product> = listing
            .product_ids
            .iter()
            .filter_map(|product_id| self.products.get(product_id))
            .collect();
        let quick_launch = products.iter().any(|product| {
            product
                .fulfillment_options
                .iter()
                .any(FulfillmentOptionRecord::supports_quick_launch)
        });
        let deployed_on_aws = products
            .iter()
            .any(|product| product.deployed_on_aws() == "DEPLOYED");
        let multi_product = listing.product_ids.len() > 1;
        let free_trial = listing.offer_ids.iter().any(|offer_id| {
            self.offer_terms.get(offer_id).is_some_and(|terms| {
                terms
                    .iter()
                    .any(|term| matches!(term, OfferTerm::freeTrialPricingTerm(_)))
            })
        });
        let free_tier = listing
            .pricing_models
            .iter()
            .any(|model| model.pricing_model_type == "FREE");

        LISTING_BADGE_DISPLAY_NAMES
            .iter()
            .filter(|(badge_type, _)| match *badge_type {
                "AWS_FREE_TIER" => free_tier,
                "DEPLOYED_ON_AWS" => deployed_on_aws,
                "FREE_TRIAL" => free_trial,
                "MULTI_PRODUCT" => multi_product,
                "QUICK_LAUNCH" => quick_launch,
                // A badge type the model gains later is not claimed until its attribute is
                // named here.
                _ => false,
            })
            .map(|(badge_type, display_name)| ListingBadge {
                badge_type: (*badge_type).to_string(),
                display_name: (*display_name).to_string(),
            })
            .collect()
    }

    /// The badges an OFFER reports: one per special attribute the offer's OWN record has,
    /// in [`PURCHASE_OPTION_BADGE_DISPLAY_NAMES`] order.
    ///
    /// `GetOfferOutput$badges` is "Badges indicating special attributes of the offer, such
    /// as private pricing, future dated, or replacement offer" and
    /// `PurchaseOptionSummary$badges` says the same of a purchase option — so a badge
    /// RESTATES something the stored offer already says, and is derived from it rather than
    /// stored beside it, where it could claim an attribute the offer does not have (an
    /// `AUTO_RENEW` badge on an offer with no renewal term, say) or omit one it does.
    ///
    /// What each badge type restates:
    ///
    /// * `PRIVATE_PRICING` — the offer's visibility scope is `PRIVATE`, the scope
    ///   `PurchaseOptionFilter$filterValues` calls "private offers and offer sets visible
    ///   to you".
    /// * `FUTURE_DATED` — the offer's `availableFromTime` ("when the purchase option became
    ///   available to the buyer") is still ahead of `now_secs`.
    /// * `REPLACEMENT_OFFER` — the offer carries a `replacementAgreementId`, "the identifier
    ///   of the existing agreement that this offer would replace".
    /// * `AUTO_RENEW` — the offer carries a `renewalTerm`, which "enables automatic agreement
    ///   renewal" (`smithy_query(query="shape",shape="RenewalTerm")`).
    pub fn offer_badges(&self, offer: &Offer, now_secs: i64) -> Vec<PurchaseOptionBadge> {
        purchase_option_badges(
            offer.visibility_scope == VISIBILITY_PRIVATE,
            future_dated(offer.available_from_time, now_secs),
            offer.replacement_agreement_id.is_some(),
            self.renews_automatically(&offer.offer_id),
        )
    }

    /// The badges an OFFER SET reports, derived the same way as an offer's.
    ///
    /// The set's own record carries its visibility scope and availability date, so
    /// `PRIVATE_PRICING` and `FUTURE_DATED` come from it. `REPLACEMENT_OFFER` and
    /// `AUTO_RENEW` are properties of the offers it bundles, and a badge on the SET claims
    /// its attribute of the whole bundle — so each is reported only when every bundled offer
    /// has it, and an empty set claims neither.
    pub fn offer_set_badges(
        &self,
        offer_set: &OfferSet,
        now_secs: i64,
    ) -> Vec<PurchaseOptionBadge> {
        let offers: Vec<&Offer> = offer_set
            .offer_ids
            .iter()
            .filter_map(|offer_id| self.offers.get(offer_id))
            .collect();
        let every_offer = |predicate: &dyn Fn(&Offer) -> bool| {
            !offers.is_empty() && offers.iter().all(|offer| predicate(offer))
        };
        purchase_option_badges(
            offer_set.visibility_scope == VISIBILITY_PRIVATE,
            future_dated(offer_set.available_from_time, now_secs),
            every_offer(&|offer| offer.replacement_agreement_id.is_some()),
            every_offer(&|offer| self.renews_automatically(&offer.offer_id)),
        )
    }

    /// Whether the offer's terms include a renewal term, the term that "enables automatic
    /// agreement renewal".
    fn renews_automatically(&self, offer_id: &str) -> bool {
        self.offer_terms.get(offer_id).is_some_and(|terms| {
            terms
                .iter()
                .any(|term| matches!(term, OfferTerm::renewalTerm(_)))
        })
    }

    /// The fulfillment option summaries a LISTING reports: one per distinct fulfillment
    /// option type its associated products offer, in product order.
    ///
    /// A listing is fulfilled through its products, so its summaries are the de-duplicated
    /// union of theirs rather than a second record that could drift from them.
    pub fn listing_fulfillment_option_summaries(
        &self,
        listing: &Listing,
    ) -> Vec<FulfillmentOptionSummary> {
        let mut summaries: Vec<FulfillmentOptionSummary> = Vec::new();
        for product_id in &listing.product_ids {
            let Some(product) = self.products.get(product_id) else {
                continue;
            };
            for summary in product.fulfillment_option_summaries() {
                if !summaries
                    .iter()
                    .any(|seen| seen.fulfillment_option_type == summary.fulfillment_option_type)
                {
                    summaries.push(summary);
                }
            }
        }
        summaries
    }

    /// Summary information about a stored product, or `None` when no such product exists.
    pub fn product_information(
        &self,
        product_id: &str,
        preferred: &'static str,
    ) -> Option<ProductInformation> {
        let product = self.products.get(product_id)?;
        let locale = product.text.content_locale(preferred);
        Some(ProductInformation {
            product_id: product.product_id.clone(),
            product_name: product.text.text(locale).name.clone(),
            manufacturer: product.manufacturer.clone(),
        })
    }

    /// Summary information about a stored offer, or `None` when no such offer exists.
    pub fn offer_information(
        &self,
        offer_id: &str,
        preferred: &'static str,
    ) -> Option<OfferInformation> {
        let offer = self.offers.get(offer_id)?;
        let locale = offer.content_locale(preferred);
        Some(OfferInformation {
            offer_id: offer.offer_id.clone(),
            offer_name: offer.name(locale),
            seller_of_record: offer.seller_of_record.clone(),
        })
    }

    /// Summary information about a stored offer set, or `None` when no such set exists.
    pub fn offer_set_information(&self, offer_set_id: &str) -> Option<OfferSetInformation> {
        let offer_set = self.offer_sets.get(offer_set_id)?;
        Some(OfferSetInformation {
            offer_set_id: offer_set.offer_set_id.clone(),
            seller_of_record: offer_set.seller_of_record.clone(),
        })
    }

    /// Every purchase option in the catalog, ordered by `purchaseOptionId`.
    ///
    /// A purchase option is not a record of its own: the model defines exactly two kinds —
    /// `OFFER` and `OFFERSET` (`PurchaseOptionType`) — so each row is projected from the
    /// stored offer or offer set it names, and `availabilityStatus` is derived from
    /// `expirationTime` against `now_secs`.
    pub fn purchase_options(
        &self,
        now_secs: i64,
        preferred: &'static str,
    ) -> Vec<PurchaseOptionEntry> {
        let mut entries: Vec<PurchaseOptionEntry> = Vec::new();

        for offer in self.offers.values() {
            let product_ids = offer.product_ids.clone();
            entries.push(PurchaseOptionEntry {
                summary: PurchaseOptionSummary {
                    purchase_option_id: offer.offer_id.clone(),
                    catalog: self.catalog.clone(),
                    purchase_option_type: "OFFER".to_string(),
                    purchase_option_name: offer.name(offer.content_locale(preferred)),
                    available_from_time: offer.available_from_time.map(datetime),
                    expiration_time: offer.expiration_time.map(datetime),
                    seller_of_record: offer.seller_of_record.clone(),
                    badges: Some(self.offer_badges(offer, now_secs)),
                    associated_entities: self.offer_associated_entities(offer, preferred),
                },
                product_ids,
                seller_profile_id: offer.seller_of_record.seller_profile_id.clone(),
                visibility_scope: offer.visibility_scope.clone(),
                availability_status: availability_status(offer.expiration_time, now_secs)
                    .to_string(),
            });
        }

        for offer_set in self.offer_sets.values() {
            let mut product_ids: Vec<String> = Vec::new();
            let mut associated_entities: Vec<PurchaseOptionAssociatedEntity> = Vec::new();
            for offer_id in &offer_set.offer_ids {
                let Some(offer) = self.offers.get(offer_id) else {
                    continue;
                };
                for entity in self.offer_associated_entities(offer, preferred) {
                    if !product_ids.contains(&entity.product.product_id) {
                        product_ids.push(entity.product.product_id.clone());
                    }
                    associated_entities.push(entity);
                }
            }
            entries.push(PurchaseOptionEntry {
                summary: PurchaseOptionSummary {
                    purchase_option_id: offer_set.offer_set_id.clone(),
                    catalog: self.catalog.clone(),
                    purchase_option_type: "OFFERSET".to_string(),
                    purchase_option_name: offer_set.offer_set_name.clone(),
                    available_from_time: offer_set.available_from_time.map(datetime),
                    expiration_time: offer_set.expiration_time.map(datetime),
                    seller_of_record: offer_set.seller_of_record.clone(),
                    badges: Some(self.offer_set_badges(offer_set, now_secs)),
                    associated_entities,
                },
                product_ids,
                seller_profile_id: offer_set.seller_of_record.seller_profile_id.clone(),
                visibility_scope: offer_set.visibility_scope.clone(),
                availability_status: availability_status(offer_set.expiration_time, now_secs)
                    .to_string(),
            });
        }

        entries.sort_by(|a, b| {
            a.summary
                .purchase_option_id
                .cmp(&b.summary.purchase_option_id)
        });
        entries
    }

    /// One entity per product the offer covers, each carrying the offer and its offer set.
    fn offer_associated_entities(
        &self,
        offer: &Offer,
        preferred: &'static str,
    ) -> Vec<PurchaseOptionAssociatedEntity> {
        let offer_information = OfferInformation {
            offer_id: offer.offer_id.clone(),
            offer_name: offer.name(offer.content_locale(preferred)),
            seller_of_record: offer.seller_of_record.clone(),
        };
        let offer_set = offer
            .offer_set_id
            .as_deref()
            .and_then(|id| self.offer_set_information(id));
        offer
            .product_ids
            .iter()
            .filter_map(|id| self.product_information(id, preferred))
            .map(|product| PurchaseOptionAssociatedEntity {
                product,
                offer: offer_information.clone(),
                offer_set: offer_set.clone(),
            })
            .collect()
    }
}

/// The badge list a purchase option reports for the attributes its record has, in
/// [`PURCHASE_OPTION_BADGE_DISPLAY_NAMES`] order.
///
/// A badge type the model gains later is not claimed until its attribute is named here.
fn purchase_option_badges(
    private_pricing: bool,
    future_dated: bool,
    replacement_offer: bool,
    auto_renew: bool,
) -> Vec<PurchaseOptionBadge> {
    PURCHASE_OPTION_BADGE_DISPLAY_NAMES
        .iter()
        .filter(|(badge_type, _)| match *badge_type {
            "PRIVATE_PRICING" => private_pricing,
            "FUTURE_DATED" => future_dated,
            "REPLACEMENT_OFFER" => replacement_offer,
            "AUTO_RENEW" => auto_renew,
            _ => false,
        })
        .map(|(badge_type, display_name)| purchase_option_badge(badge_type, display_name))
        .collect()
}

/// Whether a purchase option's availability date is still ahead of `now_secs`, which is
/// what its `FUTURE_DATED` badge restates. An option with no availability date became
/// available when it was created, so it is never future dated.
fn future_dated(available_from_time: Option<i64>, now_secs: i64) -> bool {
    available_from_time.is_some_and(|available_from| available_from > now_secs)
}

/// `AVAILABLE` until the expiration instant passes, `EXPIRED` after it; an option with no
/// expiration never expires.
///
/// Source: `PurchaseOptionFilter$filterValues` documentation — `AVAILABILITY_STATUS` is
/// `AVAILABLE` or `EXPIRED` — and `PurchaseOptionSummary$expirationTime`: "The date and
/// time when the purchase option expires and is no longer available for procurement".
pub fn availability_status(expiration_time: Option<i64>, now_secs: i64) -> &'static str {
    match expiration_time {
        Some(expires_at) if expires_at <= now_secs => "EXPIRED",
        _ => "AVAILABLE",
    }
}

/// The message this service's `ResourceNotFoundException` carries for a resource the
/// catalog does not hold: `"<Resource> not found"`, and nothing else.
///
/// The message is request-INDEPENDENT. Recorded behavior establishes it: three
/// independent recordings of `GetListing` with three DIFFERENT well-formed but unknown
/// `listingId`s all answered the identical message "Listing not found", so the message
/// names only the kind of resource that was missing and discloses neither the requested
/// id nor the catalog it was looked up in.
///
/// `id_member` is the input member the lookup was made by (`"listingId"`,
/// `"offerSetId"`), and the resource is named [`resource_name`] spells it — the model's
/// own name for the resource, never English prose about it.
pub fn not_found_message(id_member: &str) -> String {
    format!("{} not found", resource_name(id_member))
}

/// The resource an id member identifies, named as the model names it: the member's name
/// without its `Id` suffix, in the upper-camel spelling of the shape family around it —
/// `offerSetId` gives `OfferSet`, the spelling of `OfferSetId` and `GetOfferSet`, and
/// `listingId` gives `Listing`.
///
/// Derived from the member name rather than written out per operation, so a refusal can
/// no longer re-spell a resource as prose ("Offer set"): the only name available here is
/// the modelled one. A member that does not end in `Id` is a member no lookup is made by,
/// and is named as it stands.
pub fn resource_name(id_member: &str) -> String {
    let name = id_member.strip_suffix("Id").unwrap_or(id_member);
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => id_member.to_string(),
    }
}

/// A wire timestamp from stored epoch seconds.
pub fn datetime(secs: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(secs)
}

/// The listing's average customer rating, derived from its review summary.
///
/// `ReviewSummary` carries one entry per review source, each with an `averageRating` and a
/// `totalReviews`, so the listing-level rating `sortBy=AVERAGE_CUSTOMER_RATING` and the
/// `MIN`/`MAX_AVERAGE_CUSTOMER_RATING` filters order by is the review-count-weighted mean
/// of those sources. A listing with no reviews rates 0.
pub fn average_customer_rating(review_summary: Option<&ReviewSummary>) -> f64 {
    let Some(summary) = review_summary else {
        return 0.0;
    };
    let mut weighted = 0.0_f64;
    let mut reviews = 0.0_f64;
    for source in &summary.review_source_summaries {
        let Ok(rating) = source.average_rating.parse::<f64>() else {
            continue;
        };
        let count = if source.total_reviews > 0 {
            source.total_reviews as f64
        } else {
            continue;
        };
        weighted += rating * count;
        reviews += count;
    }
    if reviews > 0.0 {
        weighted / reviews
    } else {
        0.0
    }
}

/// Resolve the locale a request asked for, before content availability is considered.
///
/// Per the `Locale` documentation: the value is one or two comma-separated BCP 47 tags,
/// `en` resolves to the service default `en-US`, an absent value means the default locale,
/// and an unsupported preferred tag with no supported fallback is a `ValidationException`.
/// The `Err` is the refusal message.
pub fn resolve_requested_locale(requested: Option<&str>) -> Result<&'static str, String> {
    let Some(requested) = requested else {
        return Ok(DEFAULT_LOCALE);
    };
    let mut tags = requested.split(',').map(str::trim).filter(|t| !t.is_empty());
    let Some(preferred) = tags.next() else {
        return Ok(DEFAULT_LOCALE);
    };
    if let Some(canonical) = canonical_locale(preferred) {
        return Ok(canonical);
    }
    // An unsupported preferred tag falls back to the second tag when that one is
    // supported, and is refused when it is not.
    match tags.next().and_then(canonical_locale) {
        Some(fallback) => Ok(fallback),
        None => Err(format!(
            "locale '{preferred}' is not a supported locale and no supported fallback locale was provided; supported locales are {}",
            SUPPORTED_LOCALES.join(", ")
        )),
    }
}

/// The supported locale a BCP 47 tag resolves to, or `None` when none does.
pub fn canonical_locale(tag: &str) -> Option<&'static str> {
    let tag = tag.trim();
    if tag.eq_ignore_ascii_case("en") {
        return Some(DEFAULT_LOCALE);
    }
    SUPPORTED_LOCALES
        .iter()
        .copied()
        .find(|supported| supported.eq_ignore_ascii_case(tag))
}

/// Encode an opaque page cursor: base64 of `<offset>:<query identity>`.
///
/// Base64's standard alphabet is exactly the `NextToken` pattern `^[a-zA-Z0-9+/=]+$`.
/// Carrying the query identity is what lets [`decode_page_token`] refuse a token minted
/// for a different query instead of silently answering from the wrong result set.
pub fn encode_page_token(query_identity: &str, offset: usize) -> String {
    ::aws_smithy_types::base64::encode(format!("{offset}:{query_identity}"))
}

/// Decode a cursor minted by [`encode_page_token`] for the same query.
///
/// The `Err` is the refusal message: a token the service did not mint, or one minted for
/// another query, is bad input rather than a reason to silently restart at page 1.
pub fn decode_page_token(token: &str, query_identity: &str) -> Result<usize, String> {
    const MALFORMED: &str = "nextToken is not a pagination token issued by this service";
    let decoded = ::aws_smithy_types::base64::decode(token).map_err(|_| MALFORMED.to_string())?;
    let decoded = String::from_utf8(decoded).map_err(|_| MALFORMED.to_string())?;
    let (offset, identity) = decoded.split_once(':').ok_or_else(|| MALFORMED.to_string())?;
    let offset: usize = offset.parse().map_err(|_| MALFORMED.to_string())?;
    if identity != query_identity {
        return Err(
            "nextToken was issued for a different request; page the same request or omit nextToken"
                .to_string(),
        );
    }
    Ok(offset)
}

/// One page of `items` plus the token for the next one, or the refusal message for a
/// `nextToken` this service did not mint for this query.
pub fn page<'a, T>(
    items: &'a [T],
    next_token: Option<&str>,
    query_identity: &str,
    max_results: i32,
) -> Result<(&'a [T], Option<String>), String> {
    let offset = match next_token {
        Some(token) if !token.is_empty() => decode_page_token(token, query_identity)?,
        _ => 0,
    };
    if offset > items.len() {
        return Err(
            "nextToken refers to a position beyond the end of the result set".to_string(),
        );
    }
    let page_size = usize::try_from(max_results).unwrap_or(1).max(1);
    let end = offset.saturating_add(page_size).min(items.len());
    let token = (end < items.len()).then(|| encode_page_token(query_identity, end));
    Ok((&items[offset..end], token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState>)
}

// ---------------------------------------------------------------------------
// A member's modelled `@pattern` holds over the WHOLE value.
// ---------------------------------------------------------------------------

// Smithy specifies `@pattern` as a *find* — the expression has to match SOMEWHERE in the
// value — and the framework's input walk implements exactly that
// (`vendor/smithy-schema-server/src/constraints.rs`: "Smithy pattern semantics are a
// *find* … which is exactly `regex::Regex::is_match`"). AWS enforces the member's pattern
// over the whole value instead. For an expression a model ANCHORS with `^` / `$` the two
// readings coincide, so the walk already refuses exactly what AWS refuses, and every
// pattern-bearing member of this service is anchored
// (`smithy_query(query="shape",shape="ListingId,ProductId,OfferId,OfferSetId,Locale,NextToken,SearchFilterValue,PurchaseOptionFilterValue")`)
// except ONE: `SearchText`'s `[^\n]*`
// (`smithy_query(query="shape",shape="SearchText")`), which is *found* in every value
// there is — including one carrying a line feed, which as a whole it does not match. So
// `searchText`, the member `SearchFacetsInput` and `SearchListingsInput` both carry, is
// the member whose pattern this service has to check itself.
//
// NOTHING about constraint checking is reimplemented here: the member's own modelled
// expression is read off its schema, anchored, and re-declared in the framework's own form
// (a [`TraitMap`] carrying a [`PatternTrait`]) hung on a member schema of the same name,
// and the check is [`validate`] — the same walk the request boundary runs. Only the WORDING
// of the refusal is this service's own: recorded behavior answers this one with a sentence
// of its own rather than with a constraint clause (see [`validate_search_text`]).

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, PatternTrait};
use std::sync::LazyLock;

/// This service's Smithy namespace, for the shadow member schema below.
const NAMESPACE: &str = "com.amazonaws.marketplacediscovery";

/// The `@pattern` the model declares for `searchText`, anchored at both ends so the WHOLE
/// value has to match it.
///
/// Read off the member's own schema (so it cannot drift from the model) and wrapped in a
/// non-capturing group that keeps a top-level alternation inside the anchors. `None` when
/// the model gives the member no pattern, which is nothing to check.
static ANCHORED_SEARCH_TEXT_PATTERN: LazyLock<Option<String>> = LazyLock::new(|| {
    let modelled =
        modelled_member_pattern(crate::ops::types::SEARCH_FACETS_INPUT_SCHEMA, "searchText")?;
    Some(format!("^(?:{modelled})$"))
});

/// `searchText`'s constraints for the whole-value walk: the anchored pattern, and nothing
/// else — the member's `@length` is enforced by the boundary walk already (and the expression below is borrowed from the
/// static above, which is what gives `PatternTrait::new` the `&'static str` it takes).
static SEARCH_TEXT_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    if let Some(anchored) = ANCHORED_SEARCH_TEXT_PATTERN.as_deref() {
        traits.insert(Box::new(PatternTrait::new(anchored)));
    }
    traits
});

/// The `searchText` member schema the walk reads: its name gives the violation path and its
/// trait map carries the constraint.
static M_SEARCH_TEXT: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.marketplacediscovery#WholeValueSearchText$searchText",
        NAMESPACE,
        "searchText",
    ),
    ShapeType::String,
    "searchText",
    0,
)
.with_traits(&SEARCH_TEXT_TRAITS);

/// The root of the shadow input the walk is driven over. Its member list is empty on
/// purpose: the walk visits what [`WholeValueSearchText`] WRITES, and a root struct schema
/// carries no member name, so it contributes no path segment.
static WHOLE_VALUE_SEARCH_TEXT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.marketplacediscovery#WholeValueSearchText",
        NAMESPACE,
        "WholeValueSearchText",
    ),
    ShapeType::Structure,
    &[],
);

/// One request's `searchText`, written for the validating walk the way the codec writes a
/// string member.
struct WholeValueSearchText<'a> {
    search_text: &'a str,
}

impl SerializableStruct for WholeValueSearchText<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&M_SEARCH_TEXT, self.search_text)
    }
}

/// The member whose whole-value `@pattern` this service checks itself, as the refusal
/// below names it.
const SEARCH_TEXT_MEMBER: &str = "searchText";

/// Refuse a `searchText` whose WHOLE value does not match the member's modelled
/// `@pattern`, or pass it through.
///
/// The refusal is worded by THIS SERVICE rather than reported as a constraint clause:
/// recorded behavior answers a `searchText` carrying a line feed with the sentence
/// [`must_not_contain_newlines`] builds — fixed text naming the member, with no `N
/// validation error(s) detected` envelope, no echo of the submitted value and no mention
/// of the expression. The member's modelled class admits every character but the line feed
/// (`smithy_query(query="shape",shape="SearchText")`), so a line feed is the only way the
/// whole value can fall outside it, which is exactly what that sentence states.
///
/// `Err` is that message, so the caller answers it with the modelled
/// `ValidationException` (HTTP 400). An absent `searchText` is nothing to check.
pub fn validate_search_text(search_text: Option<&str>) -> Result<(), String> {
    let Some(search_text) = search_text else {
        return Ok(());
    };
    let violations = validate(
        &WholeValueSearchText { search_text },
        &WHOLE_VALUE_SEARCH_TEXT_SCHEMA,
    );
    if violations.is_empty() {
        return Ok(());
    }
    Err(must_not_contain_newlines(SEARCH_TEXT_MEMBER))
}

/// The sentence this service refuses a member carrying a line feed with: the member's
/// name and the rule it broke, and nothing of the request beyond that name.
///
/// Public so the handlers' own refusals and a test read the same sentence.
pub fn must_not_contain_newlines(member: &str) -> String {
    format!("{member} must not contain newlines")
}

/// The `@pattern` expression the model declares for one member of `input_schema`, or
/// `None` for a member it declares none for (or does not declare at all).
///
/// Public so a test can assert that the two inputs carrying `searchText` declare the same
/// expression, which is what makes checking one of them enough.
pub fn modelled_member_pattern(input_schema: &Schema, member: &str) -> Option<&'static str> {
    let member_schema = input_schema
        .members()
        .iter()
        .find(|candidate| candidate.member_name() == Some(member))?;
    let pattern = member_schema.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(pattern.as_any().downcast_ref::<PatternTrait>()?.regex)
}

// ---------------------------------------------------------------------------
// Searching the catalog: what `SearchListings` and `SearchFacets` share.
// ---------------------------------------------------------------------------

/// The terms a `searchText` is matched by: its whitespace-separated words, each lowercased
/// and stripped of the punctuation around it, de-duplicated and in the order the request
/// wrote them.
///
/// `SearchListings` is "search by keyword … sort results by relevance"
/// (`smithy_query(query="doc",shape="SearchListings")`), so its `searchText` is a query of
/// KEYWORDS rather than one literal string to be found: a two-word query matches a listing
/// whose content carries those words apart, and ranks one carrying both above one carrying
/// either. Splitting on whitespace only is what keeps a hyphenated or dotted identifier-like
/// word one term rather than silently widening it to its parts.
pub fn search_terms(search_text: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    for word in search_text.split_whitespace() {
        let term = word
            .trim_matches(|character: char| !character.is_alphanumeric())
            .to_lowercase();
        if !term.is_empty() && !terms.contains(&term) {
            terms.push(term);
        }
    }
    terms
}

/// Refuse a `filters` list the model's `SearchFilterType` enum cannot reject on its own: an
/// unsupported filter type, a range filter carrying more than one value or a rating outside
/// the documented 0.0–5.0 range, or a `NUMBER_OF_PRODUCTS` value that is not a count.
///
/// Checked up front rather than per listing, so the refusal is a property of the REQUEST:
/// the same malformed filter is refused whether or not any listing reaches the predicate.
pub fn validate_search_filters(filters: &[SearchFilter]) -> Result<(), String> {
    for filter in filters {
        let values = &filter.filter_values;
        match filter.filter_type.as_str() {
            // Term filters take identifiers and enum values; the values themselves name
            // catalog records, and one no record carries simply matches nothing.
            "CATEGORY" | "PUBLISHER" | "FULFILLMENT_OPTION_TYPE" | "PRICING_MODEL"
            | "PRICING_UNIT" | "DEPLOYED_ON_AWS" => {}
            "NUMBER_OF_PRODUCTS" => {
                for value in values {
                    product_count(value)?;
                }
            }
            "MIN_AVERAGE_CUSTOMER_RATING" | "MAX_AVERAGE_CUSTOMER_RATING" => {
                rating_bound(&filter.filter_type, values)?;
            }
            other => {
                return Err(format!("filters: '{other}' is not a supported filterType"));
            }
        }
    }
    Ok(())
}

/// Whether a listing satisfies every search filter (AND across filters, OR within one).
///
/// Source: `SearchListingsInput$filters` / `SearchFacetsInput$filters` — "Multiple filters
/// are combined with AND logic. Multiple values within the same filter are combined with OR
/// logic."
///
/// `Err` is the refusal message for a filter value the model's `SearchFilterType` enum
/// cannot reject on its own — a range filter with more than one value, or a rating outside
/// the documented 0.0–5.0 range.
pub fn matches_every_search_filter(
    state: &ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState,
    listing: &Listing,
    filters: &[SearchFilter],
) -> Result<bool, String> {
    for filter in filters {
        let values = &filter.filter_values;
        let matched = match filter.filter_type.as_str() {
            "CATEGORY" => listing
                .categories
                .iter()
                .any(|category| values.contains(&category.category_id)),
            "PUBLISHER" => values.contains(&listing.publisher.seller_profile_id),
            "FULFILLMENT_OPTION_TYPE" => state
                .listing_fulfillment_option_summaries(listing)
                .iter()
                .any(|option| values.contains(&option.fulfillment_option_type)),
            "PRICING_MODEL" => listing
                .pricing_models
                .iter()
                .any(|model| values.contains(&model.pricing_model_type)),
            "PRICING_UNIT" => listing
                .pricing_units
                .iter()
                .any(|unit| values.contains(&unit.pricing_unit_type)),
            "DEPLOYED_ON_AWS" => listing.product_ids.iter().any(|product_id| {
                state
                    .products
                    .get(product_id)
                    .is_some_and(|product| {
                        values.iter().any(|value| value == product.deployed_on_aws())
                    })
            }),
            "NUMBER_OF_PRODUCTS" => {
                let products = listing.product_ids.len();
                let mut matched = false;
                for value in values {
                    matched |= product_count(value)? == products;
                }
                matched
            }
            // Range filters: "accept a single value between 0.0 and 5.0"
            // (SearchFilter$filterValues documentation).
            "MIN_AVERAGE_CUSTOMER_RATING" => {
                let bound = rating_bound(&filter.filter_type, values)?;
                average_customer_rating(listing.review_summary.as_ref()) >= bound
            }
            "MAX_AVERAGE_CUSTOMER_RATING" => {
                let bound = rating_bound(&filter.filter_type, values)?;
                average_customer_rating(listing.review_summary.as_ref()) <= bound
            }
            other => {
                return Err(format!("filters: '{other}' is not a supported filterType"));
            }
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The product count a `NUMBER_OF_PRODUCTS` filter value names, refusing a value that is
/// not a non-negative integer.
fn product_count(value: &str) -> Result<usize, String> {
    value.parse().map_err(|_| {
        format!("filters: NUMBER_OF_PRODUCTS expects a non-negative integer, got '{value}'")
    })
}

/// The single rating a range filter carries, refusing a value list or a rating outside the
/// documented 0.0–5.0 range.
///
/// Both refusals are built from the request and follow AWS's recorded template for a range
/// filter: they name the offending `filterType`, and the out-of-range one also carries the
/// offending value bare after `"got: "`. A value that is not a number at all is not a
/// rating between 0.0 and 5.0 either, so it is refused by the same message.
fn rating_bound(filter_type: &str, values: &[String]) -> Result<f64, String> {
    let [value] = values else {
        return Err(format!(
            "Range filter {filter_type} requires exactly one value"
        ));
    };
    let rating: f64 = value.parse().unwrap_or(f64::NAN);
    if !(0.0..=5.0).contains(&rating) {
        return Err(format!(
            "Range filter {filter_type} value must be between 0.0 and 5.0, got: {value}"
        ));
    }
    Ok(rating)
}

// ---------------------------------------------------------------------------
// The catalog's category taxonomy.
// ---------------------------------------------------------------------------

/// The catalog's category tree: one row per category as
/// `(categoryId, displayName, parent categoryId)`, the parent being `None` for a top-level
/// category.
///
/// Why the catalog owns a TREE rather than a flat list: `ListingFacet$parent` is "The
/// parent facet value for hierarchical facets, such as subcategories"
/// (`smithy_query(query="shape",shape="ListingFacet")`), so the `CATEGORY` facet is
/// hierarchical and a subcategory's facet value has to name the category it sits under.
/// `Category` itself models no parent member, so the nesting is the CATALOG's knowledge and
/// lives here, beside the records that carry the categories.
///
/// Source: the AWS Marketplace browse taxonomy — its top-level categories (Infrastructure
/// Software, Business Applications, Data Products, Machine Learning, DevOps, IoT,
/// Professional Services, Industries), the subcategories published under each of them, and
/// the sub-subcategories published under some of those (Data Analytics' Data Integration,
/// Data Preparation and Data Visualization; Financial Services Data's Market Data and
/// Reference Data). The ids are this catalog's own slugs; the nesting and the display names
/// are the taxonomy's. The tree is three levels deep, so a record classified at a leaf
/// carries a three-category path and a `CATEGORY` facet value can report a parent that
/// itself has one.
pub const CATEGORY_TAXONOMY: &[(&str, &str, Option<&str>)] = &[
    // Infrastructure Software and its subcategories.
    ("infrastructure-software", "Infrastructure Software", None),
    (
        "operating-systems",
        "Operating Systems",
        Some("infrastructure-software"),
    ),
    ("storage", "Storage", Some("infrastructure-software")),
    (
        "data-analytics",
        "Data Analytics",
        Some("infrastructure-software"),
    ),
    // Data Analytics' own subcategories: the taxonomy's third level.
    (
        "data-integration",
        "Data Integration",
        Some("data-analytics"),
    ),
    (
        "data-preparation",
        "Data Preparation",
        Some("data-analytics"),
    ),
    (
        "data-visualization",
        "Data Visualization",
        Some("data-analytics"),
    ),
    (
        "network-infrastructure",
        "Network Infrastructure",
        Some("infrastructure-software"),
    ),
    ("security", "Security", Some("infrastructure-software")),
    (
        "backup-recovery",
        "Backup and Recovery",
        Some("infrastructure-software"),
    ),
    (
        "high-performance-computing",
        "High Performance Computing",
        Some("infrastructure-software"),
    ),
    ("migration", "Migration", Some("infrastructure-software")),
    // Business Applications and its subcategories.
    ("business-applications", "Business Applications", None),
    ("crm", "CRM", Some("business-applications")),
    (
        "collaboration-productivity",
        "Collaboration and Productivity",
        Some("business-applications"),
    ),
    ("e-commerce", "eCommerce", Some("business-applications")),
    (
        "human-resources",
        "Human Resources",
        Some("business-applications"),
    ),
    (
        "it-business-management",
        "IT Business Management",
        Some("business-applications"),
    ),
    // Data Products and its subcategories.
    ("data-products", "Data Products", None),
    (
        "financial-services-data",
        "Financial Services Data",
        Some("data-products"),
    ),
    // Financial Services Data's own subcategories: the taxonomy's third level again, under
    // a different top-level category.
    (
        "market-data",
        "Market Data",
        Some("financial-services-data"),
    ),
    (
        "reference-data",
        "Reference Data",
        Some("financial-services-data"),
    ),
    (
        "healthcare-life-sciences-data",
        "Healthcare and Life Sciences Data",
        Some("data-products"),
    ),
    (
        "environmental-data",
        "Environmental Data",
        Some("data-products"),
    ),
    (
        "public-sector-data",
        "Public Sector Data",
        Some("data-products"),
    ),
    (
        "telecommunications-data",
        "Telecommunications Data",
        Some("data-products"),
    ),
    // Machine Learning and its subcategories.
    ("machine-learning", "Machine Learning", None),
    ("computer-vision", "Computer Vision", Some("machine-learning")),
    (
        "natural-language-processing",
        "Natural Language Processing",
        Some("machine-learning"),
    ),
    (
        "speech-recognition",
        "Speech Recognition",
        Some("machine-learning"),
    ),
    (
        "structured-data-ml",
        "Structured Data",
        Some("machine-learning"),
    ),
    // DevOps and its subcategories.
    ("devops", "DevOps", None),
    (
        "application-development",
        "Application Development",
        Some("devops"),
    ),
    (
        "agile-lifecycle-management",
        "Agile Lifecycle Management",
        Some("devops"),
    ),
    ("monitoring", "Monitoring", Some("devops")),
    ("source-control", "Source Control", Some("devops")),
    ("testing", "Testing", Some("devops")),
    (
        "infrastructure-as-code",
        "Infrastructure as Code",
        Some("devops"),
    ),
    // Internet of Things and its subcategories.
    ("iot", "Internet of Things", None),
    ("iot-analytics", "IoT Analytics", Some("iot")),
    (
        "iot-device-management",
        "IoT Device Management",
        Some("iot"),
    ),
    ("iot-edge-computing", "IoT Edge Computing", Some("iot")),
    // Professional Services and its subcategories.
    ("professional-services", "Professional Services", None),
    ("assessments", "Assessments", Some("professional-services")),
    (
        "implementation",
        "Implementation",
        Some("professional-services"),
    ),
    (
        "managed-services",
        "Managed Services",
        Some("professional-services"),
    ),
    ("training", "Training", Some("professional-services")),
    // Industries and its subcategories.
    ("industries", "Industries", None),
    ("financial-services", "Financial Services", Some("industries")),
    ("healthcare", "Healthcare", Some("industries")),
    (
        "media-entertainment",
        "Media and Entertainment",
        Some("industries"),
    ),
    ("retail", "Retail", Some("industries")),
    ("energy", "Energy", Some("industries")),
    (
        "telecommunications",
        "Telecommunications",
        Some("industries"),
    ),
    (
        "education-research",
        "Education and Research",
        Some("industries"),
    ),
    ("public-sector", "Public Sector", Some("industries")),
];

/// The taxonomy row for `category_id`, or `None` for an id the taxonomy does not hold.
fn taxonomy_row(category_id: &str) -> Option<&'static (&'static str, &'static str, Option<&'static str>)>
{
    CATEGORY_TAXONOMY.iter().find(|(id, _, _)| *id == category_id)
}

/// The category `category_id` sits under, or `None` when it is top-level (or unknown).
///
/// This is the value `ListingFacet$parent` carries for a `CATEGORY` facet value: the
/// PARENT'S value, which is itself a `CATEGORY` filter value.
pub fn category_parent(category_id: &str) -> Option<&'static str> {
    taxonomy_row(category_id).and_then(|(_, _, parent)| *parent)
}

/// The category record for `category_id`, with the taxonomy's display name.
///
/// A `categoryId` the taxonomy does not hold is displayed by its own id, which keeps
/// `Category$displayName` (required, non-blank) satisfiable for a record classified under a
/// category added to the catalog before the taxonomy names it.
pub fn taxonomy_category(category_id: &str) -> Category {
    Category {
        category_id: category_id.to_string(),
        display_name: taxonomy_row(category_id)
            .map_or_else(|| category_id.to_string(), |(_, name, _)| (*name).to_string()),
    }
}

/// The categories a record classified under `leaves` carries: each leaf's taxonomy PATH,
/// outermost ancestor first and the leaf last, with no category repeated.
///
/// A record classified under a subcategory is classified under that subcategory's parents
/// too — browsing "Infrastructure Software" finds what is listed under "Storage" — so the
/// path, not the leaf alone, is what a listing or product carries.
pub fn classified_under(leaves: &[&str]) -> Vec<Category> {
    let mut path: Vec<Category> = Vec::new();
    for leaf in leaves {
        // Ancestors of this leaf, outermost first.
        let mut ancestors: Vec<&str> = Vec::new();
        let mut cursor = category_parent(leaf);
        while let Some(parent) = cursor {
            if ancestors.contains(&parent) {
                break;
            }
            ancestors.insert(0, parent);
            cursor = category_parent(parent);
        }
        ancestors.push(leaf);
        for category_id in ancestors {
            if path.iter().any(|held| held.category_id == category_id) {
                continue;
            }
            path.push(taxonomy_category(category_id));
        }
    }
    path
}

// ---------------------------------------------------------------------------
// The seeded catalog.
// ---------------------------------------------------------------------------

fn seller(profile_id: &str, display_name: &str) -> SellerInformation {
    SellerInformation {
        seller_profile_id: profile_id.to_string(),
        display_name: display_name.to_string(),
    }
}

/// An AWS service a fulfillment option can be deployed to or integrated with.
fn aws_service(service_type: &str, display_name: &str, description: &str) -> AwsSupportedService {
    AwsSupportedService {
        supported_service_type: service_type.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
    }
}

/// A SaaS fulfillment option: the seller hosts the software and the buyer reaches it over
/// the web, so the option carries registration and launch URLs rather than a version.
fn saas_option(
    id: &str,
    fulfillment_url: &str,
    launch_url: &str,
    quick_launch: &str,
    usage_instructions: &str,
    available_from: i64,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::saasFulfillmentOption(
        SaasFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "SAAS".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("SAAS"),
            fulfillment_url: Some(fulfillment_url.to_string()),
            usage_instructions: Some(usage_instructions.to_string()),
            available_from_time: Some(datetime(available_from)),
            launch_url: Some(launch_url.to_string()),
            quick_launch: quick_launch.to_string(),
        },
    ))
}

/// An API fulfillment option: the buyer calls the seller's API, so the option names the
/// AWS services the integration is supported on.
fn api_option(
    id: &str,
    usage_instructions: &str,
    services: Vec<AwsSupportedService>,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::apiFulfillmentOption(
        ApiFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "API".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("API"),
            usage_instructions: Some(usage_instructions.to_string()),
            aws_supported_services: services,
        },
    ))
}

/// An Amazon Machine Image fulfillment option, with the operating systems it ships and
/// the instance type and EBS volumes recommended for running it.
fn ami_option(
    id: &str,
    name: &str,
    version: &str,
    architecture: &str,
    usage_instructions: &str,
    instance_type: &str,
    available_from: i64,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::amazonMachineImageFulfillmentOption(
        AmazonMachineImageFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_name: name.to_string(),
            fulfillment_option_version: Some(version.to_string()),
            fulfillment_option_type: "AMAZON_MACHINE_IMAGE".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name(
                "AMAZON_MACHINE_IMAGE",
            ),
            operating_systems: vec![AmazonMachineImageOperatingSystem {
                operating_system_family_name: "AMAZONLINUX".to_string(),
                operating_system_name: "Amazon Linux".to_string(),
                operating_system_version: Some("2023".to_string()),
            }],
            recommendation: Some(AmazonMachineImageRecommendation {
                instance_type: instance_type.to_string(),
                security_groups: Some(vec![AmazonMachineImageSecurityGroup {
                    protocol: "tcp".to_string(),
                    from_port: 443,
                    to_port: 443,
                    cidr_ip_addresses: vec!["0.0.0.0/0".to_string()],
                }]),
            }),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
            available_from_time: Some(datetime(available_from)),
            access_url_template: Some("https://{instance_dns}/".to_string()),
            architecture: architecture.to_string(),
            ami_alias: Some(format!("resolve:ssm:/aws/marketplace/{id}")),
            ebs_volume: Some(AmazonMachineImageEbsVolume {
                volume_types: vec!["gp3".to_string(), "io2".to_string()],
                iops: Some(3000),
            }),
            short_description: Some(format!("{name}, as an Amazon Machine Image.")),
        },
    ))
}

/// An AWS CloudFormation template fulfillment option.
fn cloudformation_option(
    id: &str,
    name: &str,
    version: &str,
    usage_instructions: &str,
    short_description: &str,
    available_from: i64,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::cloudFormationFulfillmentOption(
        CloudFormationFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_name: name.to_string(),
            fulfillment_option_type: "CLOUDFORMATION_TEMPLATE".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name(
                "CLOUDFORMATION_TEMPLATE",
            ),
            fulfillment_option_version: Some(version.to_string()),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
            available_from_time: Some(datetime(available_from)),
            short_description: Some(short_description.to_string()),
            long_description: Some(format!(
                "{short_description} The template provisions every resource the product \
                 needs in the buyer's own account."
            )),
        },
    ))
}

/// A container image fulfillment option, deployable to the container services it names.
fn container_option(
    id: &str,
    name: &str,
    version: &str,
    usage_instructions: &str,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::containerFulfillmentOption(
        ContainerFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_name: name.to_string(),
            fulfillment_option_type: "CONTAINER".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("CONTAINER"),
            fulfillment_option_version: Some(version.to_string()),
            operating_systems: Some(vec![ContainerOperatingSystem {
                operating_system_family_name: "DEBIAN".to_string(),
                operating_system_name: "Debian".to_string(),
            }]),
            aws_supported_services: Some(vec![
                aws_service("EKS", "Amazon EKS", "Elastic Kubernetes Service"),
                aws_service("ECS", "Amazon ECS", "Elastic Container Service"),
            ]),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
        },
    ))
}

/// A Helm chart fulfillment option, installed into the buyer's Kubernetes cluster.
fn helm_option(
    id: &str,
    name: &str,
    version: &str,
    usage_instructions: &str,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::helmFulfillmentOption(
        HelmFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_name: name.to_string(),
            fulfillment_option_type: "HELM".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("HELM"),
            fulfillment_option_version: Some(version.to_string()),
            operating_systems: Some(vec![HelmOperatingSystem {
                operating_system_family_name: "DEBIAN".to_string(),
                operating_system_name: "Debian".to_string(),
            }]),
            release_notes: Some(format!("Release notes for version {version}.")),
            aws_supported_services: Some(vec![aws_service(
                "EKS",
                "Amazon EKS",
                "Elastic Kubernetes Service",
            )]),
            usage_instructions: Some(usage_instructions.to_string()),
        },
    ))
}

/// An Amazon EKS add-on fulfillment option.
fn eks_add_on_option(
    id: &str,
    name: &str,
    version: &str,
    usage_instructions: &str,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::eksAddOnFulfillmentOption(
        EksAddOnFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_name: name.to_string(),
            fulfillment_option_type: "EKS_ADD_ON".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("EKS_ADD_ON"),
            fulfillment_option_version: Some(version.to_string()),
            operating_systems: Some(vec![EksAddOnOperatingSystem {
                operating_system_family_name: "AMAZONLINUX".to_string(),
                operating_system_name: "Amazon Linux".to_string(),
            }]),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
            aws_supported_services: Some(vec![aws_service(
                "EKS",
                "Amazon EKS",
                "Elastic Kubernetes Service",
            )]),
        },
    ))
}

/// An EC2 Image Builder component fulfillment option.
fn ec2_image_builder_option(
    id: &str,
    name: &str,
    version: &str,
    usage_instructions: &str,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(
        FulfillmentOption::ec2ImageBuilderComponentFulfillmentOption(
            Ec2ImageBuilderComponentFulfillmentOption {
                fulfillment_option_id: id.to_string(),
                fulfillment_option_name: name.to_string(),
                fulfillment_option_type: "EC2_IMAGE_BUILDER_COMPONENT".to_string(),
                fulfillment_option_display_name: fulfillment_option_display_name(
                    "EC2_IMAGE_BUILDER_COMPONENT",
                ),
                fulfillment_option_version: Some(version.to_string()),
                operating_systems: Some(vec![ContainerOperatingSystem {
                    operating_system_family_name: "AMAZONLINUX".to_string(),
                    operating_system_name: "Amazon Linux".to_string(),
                }]),
                aws_supported_services: Some(vec![aws_service(
                    "IMAGEBUILDER",
                    "EC2 Image Builder",
                    "Image pipeline service",
                )]),
                release_notes: Some(format!("Release notes for version {version}.")),
                usage_instructions: Some(usage_instructions.to_string()),
            },
        ),
    )
}

/// An AWS Data Exchange fulfillment option, delivering the data sets it names.
fn data_exchange_option(id: &str, data_set_arn: &str, description: &str) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::dataExchangeFulfillmentOption(
        DataExchangeFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "DATA_EXCHANGE".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("DATA_EXCHANGE"),
            data_artifacts: Some(vec![DataArtifact {
                description: Some(description.to_string()),
                resource_arn: Some(data_set_arn.to_string()),
                resource_type: "DATA_SET".to_string(),
                data_classification: "PUBLIC".to_string(),
            }]),
        },
    ))
}

/// A professional services fulfillment option, which the model gives no deployment detail
/// beyond its identity and type.
fn professional_services_option(id: &str) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::professionalServicesFulfillmentOption(
        ProfessionalServicesFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "PROFESSIONAL_SERVICES".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name(
                "PROFESSIONAL_SERVICES",
            ),
        },
    ))
}

/// A SageMaker algorithm fulfillment option, with the instance types recommended for
/// training and for inference.
fn sagemaker_algorithm_option(
    id: &str,
    version: &str,
    usage_instructions: &str,
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::sageMakerAlgorithmFulfillmentOption(
        SageMakerAlgorithmFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "SAGEMAKER_ALGORITHM".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("SAGEMAKER_ALGORITHM"),
            fulfillment_option_version: Some(version.to_string()),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
            recommendation: Some(SageMakerAlgorithmRecommendation {
                recommended_batch_transform_instance_type: "ml.m5.xlarge".to_string(),
                recommended_realtime_inference_instance_type: Some("ml.g5.xlarge".to_string()),
                recommended_training_instance_type: "ml.p3.2xlarge".to_string(),
            }),
        },
    ))
}

/// A SageMaker model fulfillment option, with the content types its endpoint accepts.
fn sagemaker_model_option(
    id: &str,
    version: &str,
    usage_instructions: &str,
    content_types: &[&str],
) -> FulfillmentOptionRecord {
    FulfillmentOptionRecord::new(FulfillmentOption::sageMakerModelFulfillmentOption(
        SageMakerModelFulfillmentOption {
            fulfillment_option_id: id.to_string(),
            fulfillment_option_type: "SAGEMAKER_MODEL".to_string(),
            fulfillment_option_display_name: fulfillment_option_display_name("SAGEMAKER_MODEL"),
            fulfillment_option_version: Some(version.to_string()),
            release_notes: Some(format!("Release notes for version {version}.")),
            usage_instructions: Some(usage_instructions.to_string()),
            recommendation: Some(SageMakerModelRecommendation {
                recommended_batch_transform_instance_type: "ml.m5.xlarge".to_string(),
                recommended_realtime_inference_instance_type: Some("ml.g5.xlarge".to_string()),
            }),
            supported_content_types: Some(
                content_types.iter().map(|t| (*t).to_string()).collect(),
            ),
            supported_response_mime_types: Some(vec!["application/json".to_string()]),
        },
    ))
}

fn pricing_model(model_type: &str, display_name: &str) -> PricingModel {
    PricingModel {
        pricing_model_type: model_type.to_string(),
        display_name: display_name.to_string(),
    }
}

fn pricing_unit(unit_type: &str, display_name: &str) -> PricingUnit {
    PricingUnit {
        pricing_unit_type: unit_type.to_string(),
        display_name: display_name.to_string(),
    }
}

fn purchase_option_badge(badge_type: &str, display_name: &str) -> PurchaseOptionBadge {
    PurchaseOptionBadge {
        display_name: display_name.to_string(),
        badge_type: badge_type.to_string(),
    }
}

fn reviews(average_rating: &str, total_reviews: i64) -> Option<ReviewSummary> {
    Some(ReviewSummary {
        review_source_summaries: vec![ReviewSourceSummary {
            source_name: "AWS Marketplace".to_string(),
            source_id: "AWS_MARKETPLACE".to_string(),
            source_url: Some("https://aws.amazon.com/marketplace/reviews".to_string()),
            average_rating: average_rating.to_string(),
            total_reviews,
        }],
    })
}

fn text(name: &str, short_description: &str, long_description: &str) -> LocalizedText {
    LocalizedText {
        name: name.to_string(),
        short_description: short_description.to_string(),
        long_description: long_description.to_string(),
    }
}

/// The resources a record carries: how to reach its manufacturer, and how to use it.
///
/// `ResourceType` declares exactly two kinds — `MANUFACTURER_SUPPORT` and
/// `MANUFACTURER_INSTRUCTIONS` — and `ResourceContentType` four formats — `EMAIL`,
/// `PHONE_NUMBER`, `LINK` and `OTHER` — `Resource$contentType` being "[t]he format of the
/// resource content, such as a URL, email address, or text"
/// (`smithy_query(query="shape",shape="Resource")`). A record therefore carries one resource
/// per kind of contact and instruction the catalog holds for it, and between them the four
/// span every format the model declares: a client that branches on `contentType` to dial a
/// number or to render text rather than follow a link has a record to read, which it has not
/// when a catalog only ever emits an address and a link.
///
/// The telephone numbers the callers pass are in the `555-01xx` range reserved for
/// fictional use.
fn support_resources(support_email: &str, support_phone: &str, docs_url: &str) -> Vec<Resource> {
    vec![
        Resource {
            resource_type: "MANUFACTURER_SUPPORT".to_string(),
            content_type: "EMAIL".to_string(),
            value: support_email.to_string(),
            display_name: Some("Support email".to_string()),
        },
        Resource {
            resource_type: "MANUFACTURER_SUPPORT".to_string(),
            content_type: "PHONE_NUMBER".to_string(),
            value: support_phone.to_string(),
            display_name: Some("Support telephone".to_string()),
        },
        Resource {
            resource_type: "MANUFACTURER_INSTRUCTIONS".to_string(),
            content_type: "LINK".to_string(),
            value: docs_url.to_string(),
            display_name: Some("Documentation".to_string()),
        },
        Resource {
            resource_type: "MANUFACTURER_INSTRUCTIONS".to_string(),
            content_type: "OTHER".to_string(),
            value: format!(
                "Subscribe, then follow the getting-started steps of the documentation at \
                 {docs_url}."
            ),
            display_name: Some("Getting started".to_string()),
        },
    ]
}

fn engagements(form_url: &str) -> Vec<SellerEngagement> {
    vec![
        SellerEngagement {
            engagement_type: "REQUEST_FOR_PRIVATE_OFFER".to_string(),
            content_type: "LINK".to_string(),
            value: form_url.to_string(),
        },
        SellerEngagement {
            engagement_type: "REQUEST_FOR_DEMO".to_string(),
            content_type: "LINK".to_string(),
            value: format!("{form_url}/demo"),
        },
    ]
}

fn promotional_image(title: &str, url: &str, description: &str) -> PromotionalMedia {
    PromotionalMedia::embeddedImage(PromotionalEmbeddedImage {
        title: title.to_string(),
        url: url.to_string(),
        description: Some(description.to_string()),
    })
}

fn promotional_video(title: &str, url: &str) -> PromotionalMedia {
    PromotionalMedia::embeddedVideo(PromotionalEmbeddedVideo {
        title: title.to_string(),
        url: url.to_string(),
        preview: format!("{url}/preview"),
        thumbnail: format!("{url}/thumbnail"),
        description: None,
    })
}

fn use_case(value: &str, display_name: &str, description: &str) -> UseCaseEntry {
    UseCaseEntry {
        use_case: UseCase {
            description: description.to_string(),
            display_name: display_name.to_string(),
            value: value.to_string(),
        },
    }
}

fn validity_term(id: &str, duration: &str, start: i64, end: i64) -> OfferTerm {
    OfferTerm::validityTerm(ValidityTerm {
        id: id.to_string(),
        r#type: "ValidityTerm".to_string(),
        agreement_duration: Some(duration.to_string()),
        agreement_start_date: Some(datetime(start)),
        agreement_end_date: Some(datetime(end)),
    })
}

fn legal_term(id: &str, document_type: &str, url: &str) -> OfferTerm {
    OfferTerm::legalTerm(LegalTerm {
        id: id.to_string(),
        r#type: "LegalTerm".to_string(),
        documents: vec![DocumentItem {
            r#type: document_type.to_string(),
            url: url.to_string(),
            version: Some("1".to_string()),
        }],
    })
}

fn support_term(id: &str, refund_policy: &str) -> OfferTerm {
    OfferTerm::supportTerm(SupportTerm {
        id: id.to_string(),
        r#type: "SupportTerm".to_string(),
        refund_policy: refund_policy.to_string(),
    })
}

/// A renewal term: the term that "enables automatic agreement renewal", and the record a
/// purchase option's `AUTO_RENEW` badge restates. `max_renewals` absent means unlimited
/// renewals (`RenewalTerm$maxRenewals`).
fn renewal_term(id: &str, max_renewals: Option<i32>) -> OfferTerm {
    OfferTerm::renewalTerm(RenewalTerm {
        id: id.to_string(),
        r#type: "RenewalTerm".to_string(),
        max_renewals,
        lockout_period: None,
        adjustment_deadline: None,
        price_increase: None,
        term_templates: None,
    })
}

fn recurring_payment_term(id: &str, price: &str) -> OfferTerm {
    OfferTerm::recurringPaymentTerm(RecurringPaymentTerm {
        id: id.to_string(),
        r#type: "RecurringPaymentTerm".to_string(),
        currency_code: "USD".to_string(),
        billing_period: "Monthly".to_string(),
        price: price.to_string(),
    })
}

/// A free-trial pricing term: the term that lets a buyer try the product before paying,
/// and the record the listing's `FREE_TRIAL` badge restates.
fn free_trial_term(id: &str, duration: &str, dimension: &str, display_name: &str, unit: &str) -> OfferTerm {
    OfferTerm::freeTrialPricingTerm(FreeTrialPricingTerm {
        id: id.to_string(),
        r#type: "FreeTrialPricingTerm".to_string(),
        duration: Some(duration.to_string()),
        grants: vec![GrantItem {
            dimension_key: dimension.to_string(),
            display_name: display_name.to_string(),
            description: None,
            dimension_labels: None,
            unit: unit.to_string(),
            max_quantity: None,
        }],
    })
}

fn usage_based_term(id: &str, dimension: &str, display_name: &str, unit: &str, price: &str) -> OfferTerm {
    OfferTerm::usageBasedPricingTerm(UsageBasedPricingTerm {
        id: id.to_string(),
        r#type: "UsageBasedPricingTerm".to_string(),
        currency_code: "USD".to_string(),
        rate_cards: vec![UsageBasedRateCardItem {
            rate_card: vec![RateCardItem {
                dimension_key: dimension.to_string(),
                display_name: display_name.to_string(),
                description: None,
                dimension_labels: None,
                unit: unit.to_string(),
                price: price.to_string(),
            }],
        }],
    })
}

/// Build the catalog the emulator serves.
///
/// Timestamps are relative to the instant the catalog is seeded, so the expired offer is
/// expired and the live ones are live however long the process has been running.
#[allow(clippy::too_many_lines)]
fn seed_catalog() -> ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
    let now = now_secs();
    let year_ago = now - 365 * DAY;
    let next_year = now + 365 * DAY;
    let month_ago = now - 30 * DAY;
    let next_month = now + 30 * DAY;

    let mut state = ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState {
        catalog: "AWSMarketplace".to_string(),
        listings: BTreeMap::new(),
        products: BTreeMap::new(),
        offers: BTreeMap::new(),
        offer_sets: BTreeMap::new(),
        offer_terms: BTreeMap::new(),
    };

    // ---- l-0001-analytics ------------------------------------------------
    let acme = seller("sp-acme", "Acme Software");
    let mut analytics_text = Localized {
        default: text(
            "Acme Analytics Suite",
            "Self-service analytics and dashboards for operational data.",
            "Acme Analytics Suite collects, models and visualises operational data. It ships \
             connectors for relational databases, object storage and streaming sources, and \
             a dashboard builder for business users.",
        ),
        translations: BTreeMap::new(),
    };
    analytics_text.translations.insert(
        "fr".to_string(),
        text(
            "Suite d'analyse Acme",
            "Analyse en libre-service et tableaux de bord pour les donnees operationnelles.",
            "La Suite d'analyse Acme collecte, modelise et visualise les donnees \
             operationnelles, avec des connecteurs pour les bases relationnelles, le stockage \
             objet et les flux.",
        ),
    );
    analytics_text.translations.insert(
        "ja".to_string(),
        text(
            "Acme Analytics Suite (JA)",
            "運用データ向けのセルフサービス分析とダッシュボード。",
            "Acme Analytics Suite は運用データを収集し、モデル化し、可視化します。",
        ),
    );
    state.listings.insert(
        "l-0001-analytics".to_string(),
        Listing {
            listing_id: "l-0001-analytics".to_string(),
            text: analytics_text,
            logo_thumbnail_url: "https://example.com/logos/acme-analytics.png".to_string(),
            integration_guide: Some("https://example.com/docs/acme-analytics/integrate".to_string()),
            categories: classified_under(&["business-applications", "data-products"]),
            highlights: vec![
                "Connects to 40 data sources".to_string(),
                "Row-level access control".to_string(),
            ],
            pricing_models: vec![
                pricing_model("CONTRACT", "Contract"),
                pricing_model("USAGE", "Usage based"),
            ],
            pricing_units: vec![pricing_unit("USERS", "Users")],
            product_ids: vec!["p-0001-analytics-core".to_string()],
            offer_ids: vec![
                "o-0001-analytics-annual".to_string(),
                "o-0002-analytics-usage".to_string(),
            ],
            publisher: acme.clone(),
            promotional_media: vec![promotional_image(
                "Dashboard builder",
                "https://example.com/media/acme-analytics/dashboard.png",
                "Building a dashboard from a modelled data source.",
            )],
            resources: support_resources(
                "support@example.com",
                "+1-206-555-0101",
                "https://example.com/docs/acme-analytics",
            ),
            seller_engagements: engagements("https://example.com/acme/contact"),
            use_cases: vec![use_case(
                "operational-reporting",
                "Operational reporting",
                "Publish scheduled operational reports to business teams.",
            )],
            review_summary: reviews("4.6", 120),
        },
    );
    let mut analytics_product_text = Localized {
        default: text(
            "Acme Analytics Core",
            "The hosted analytics engine behind the Acme Analytics Suite.",
            "Acme Analytics Core is the multi-tenant query and modelling engine that backs \
             every Acme Analytics Suite dashboard.",
        ),
        translations: BTreeMap::new(),
    };
    analytics_product_text.translations.insert(
        "fr".to_string(),
        text(
            "Acme Analytics Core (FR)",
            "Le moteur d'analyse heberge de la Suite d'analyse Acme.",
            "Acme Analytics Core est le moteur de requete et de modelisation multi-locataire.",
        ),
    );
    state.products.insert(
        "p-0001-analytics-core".to_string(),
        Product {
            product_id: "p-0001-analytics-core".to_string(),
            listing_id: "l-0001-analytics".to_string(),
            text: analytics_product_text,
            manufacturer: acme.clone(),
            logo_thumbnail_url: "https://example.com/logos/acme-analytics.png".to_string(),
            categories: classified_under(&["business-applications"]),
            highlights: vec!["Hosted by the seller".to_string()],
            promotional_media: vec![promotional_image(
                "Query engine",
                "https://example.com/media/acme-analytics/engine.png",
                "The modelling layer of Acme Analytics Core.",
            )],
            resources: support_resources(
                "support@example.com",
                "+1-206-555-0101",
                "https://example.com/docs/acme-analytics",
            ),
            seller_engagements: engagements("https://example.com/acme/contact"),
            fulfillment_options: vec![
                saas_option(
                    "fo-0001-analytics-saas",
                    "https://example.com/acme/register",
                    "https://example.com/acme/launch",
                    "ENABLED",
                    "Register with your AWS account, then invite your analysts from the Acme \
                     console.",
                    year_ago,
                )
                .translate(
                    "fr",
                    "Inscrivez-vous avec votre compte AWS, puis invitez vos analystes depuis \
                     la console Acme.",
                ),
                api_option(
                    "fo-0002-analytics-api",
                    "Call the Acme Analytics REST API with the key issued at registration.",
                    vec![aws_service(
                        "APIGATEWAY",
                        "Amazon API Gateway",
                        "Fronts the Acme Analytics REST API.",
                    )],
                ),
            ],
        },
    );
    state.offers.insert(
        "o-0001-analytics-annual".to_string(),
        Offer {
            offer_id: "o-0001-analytics-annual".to_string(),
            offer_name: Some("Acme Analytics Suite — annual contract".to_string()),
            name_translations: BTreeMap::from([(
                "fr".to_string(),
                "Suite d'analyse Acme — contrat annuel".to_string(),
            )]),
            expiration_time: Some(next_year),
            available_from_time: Some(year_ago),
            seller_of_record: acme.clone(),
            product_ids: vec!["p-0001-analytics-core".to_string()],
            offer_set_id: Some("os-0001-analytics".to_string()),
            agreement_proposal_id: "ap-0001-analytics-annual".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("CONTRACT", "Contract"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0001-analytics-annual".to_string(),
        vec![
            validity_term("t-0001-validity", "P365D", year_ago, next_year),
            free_trial_term(
                "t-0001-free-trial",
                "P14D",
                "ActiveUser",
                "Active user",
                "Users",
            ),
            recurring_payment_term("t-0001-recurring", "1200.00"),
            // An annual contract that renews automatically, which is what this offer's
            // AUTO_RENEW badge restates.
            renewal_term("t-0001-renewal", Some(3)),
            legal_term(
                "t-0001-legal",
                "StandardEula",
                "https://example.com/legal/standard-eula",
            ),
            support_term("t-0001-support", "Pro-rated refunds within 30 days."),
        ],
    );
    state.offers.insert(
        "o-0002-analytics-usage".to_string(),
        Offer {
            offer_id: "o-0002-analytics-usage".to_string(),
            offer_name: Some("Acme Analytics Suite — pay as you go".to_string()),
            name_translations: BTreeMap::new(),
            expiration_time: Some(next_year),
            available_from_time: Some(next_month),
            seller_of_record: acme.clone(),
            product_ids: vec!["p-0001-analytics-core".to_string()],
            offer_set_id: Some("os-0001-analytics".to_string()),
            agreement_proposal_id: "ap-0002-analytics-usage".to_string(),
            replacement_agreement_id: Some("ag-0001-analytics-legacy".to_string()),
            pricing_model: pricing_model("USAGE", "Usage based"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0002-analytics-usage".to_string(),
        vec![
            validity_term("t-0002-validity", "P30D", next_month, next_year),
            usage_based_term("t-0002-usage", "ActiveUser", "Active user", "Users", "12.00"),
            legal_term(
                "t-0002-legal",
                "StandardEula",
                "https://example.com/legal/standard-eula",
            ),
        ],
    );
    state.offer_sets.insert(
        "os-0001-analytics".to_string(),
        OfferSet {
            offer_set_id: "os-0001-analytics".to_string(),
            offer_set_name: Some("Acme Analytics Suite purchase options".to_string()),
            available_from_time: Some(year_ago),
            expiration_time: Some(next_year),
            buyer_notes: Some("Annual contract or pay as you go.".to_string()),
            seller_of_record: acme,
            offer_ids: vec![
                "o-0001-analytics-annual".to_string(),
                "o-0002-analytics-usage".to_string(),
            ],
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );

    // ---- l-0002-firewall -------------------------------------------------
    let nimbus = seller("sp-nimbus", "Nimbus Networks");
    let mut firewall_text = Localized {
        default: text(
            "Nimbus Cloud Firewall",
            "Stateful network firewall appliance for VPC perimeters.",
            "Nimbus Cloud Firewall inspects north-south traffic at the VPC perimeter with \
             stateful rules, intrusion detection and centralised policy management.",
        ),
        translations: BTreeMap::new(),
    };
    firewall_text.translations.insert(
        "fr".to_string(),
        text(
            "Pare-feu cloud Nimbus",
            "Pare-feu reseau avec etat pour les peripheries de VPC.",
            "Le pare-feu cloud Nimbus inspecte le trafic nord-sud a la peripherie du VPC.",
        ),
    );
    state.listings.insert(
        "l-0002-firewall".to_string(),
        Listing {
            listing_id: "l-0002-firewall".to_string(),
            text: firewall_text,
            logo_thumbnail_url: "https://example.com/logos/nimbus-firewall.png".to_string(),
            integration_guide: None,
            categories: classified_under(&["network-infrastructure"]),
            highlights: vec!["Deep packet inspection".to_string()],
            pricing_models: vec![pricing_model("USAGE", "Usage based")],
            pricing_units: vec![pricing_unit("HOSTS", "Hosts")],
            product_ids: vec!["p-0002-firewall-appliance".to_string()],
            offer_ids: vec!["o-0003-firewall-hourly".to_string()],
            publisher: nimbus.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "help@example.com",
                "+1-206-555-0102",
                "https://example.com/docs/nimbus-firewall",
            ),
            seller_engagements: engagements("https://example.com/nimbus/contact"),
            use_cases: vec![use_case(
                "perimeter-security",
                "Perimeter security",
                "Filter inbound and outbound traffic at the VPC edge.",
            )],
            review_summary: reviews("4.1", 58),
        },
    );
    state.products.insert(
        "p-0002-firewall-appliance".to_string(),
        Product {
            product_id: "p-0002-firewall-appliance".to_string(),
            listing_id: "l-0002-firewall".to_string(),
            text: Localized {
                default: text(
                    "Nimbus Firewall Appliance",
                    "The Nimbus firewall, delivered as an Amazon Machine Image.",
                    "The Nimbus Firewall Appliance runs on EC2 and is managed from the Nimbus \
                     console.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: nimbus.clone(),
            logo_thumbnail_url: "https://example.com/logos/nimbus-firewall.png".to_string(),
            categories: classified_under(&["network-infrastructure"]),
            highlights: vec!["Runs in your account".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "help@example.com",
                "+1-206-555-0102",
                "https://example.com/docs/nimbus-firewall",
            ),
            seller_engagements: engagements("https://example.com/nimbus/contact"),
            fulfillment_options: vec![
                ami_option(
                    "fo-0003-firewall-ami",
                    "Nimbus Firewall Appliance 7.2",
                    "7.2.1",
                    "x86_64",
                    "Launch the AMI into the inspection subnet, then adopt the instance from \
                     the Nimbus console.",
                    "c6i.2xlarge",
                    year_ago,
                ),
                cloudformation_option(
                    "fo-0004-firewall-cfn",
                    "Nimbus Firewall reference architecture",
                    "7.2.1",
                    "Deploy the stack into the VPC whose edge you want inspected.",
                    "A highly available pair of firewall appliances behind a Gateway Load \
                     Balancer.",
                    year_ago,
                ),
                ec2_image_builder_option(
                    "fo-0005-firewall-component",
                    "Nimbus Firewall hardening component",
                    "7.2.1",
                    "Add the component to an Image Builder recipe to bake the firewall agent \
                     into your own image.",
                ),
            ],
        },
    );
    state.offers.insert(
        "o-0003-firewall-hourly".to_string(),
        Offer {
            offer_id: "o-0003-firewall-hourly".to_string(),
            offer_name: Some("Nimbus Cloud Firewall — hourly".to_string()),
            name_translations: BTreeMap::new(),
            expiration_time: Some(next_year),
            available_from_time: Some(year_ago),
            seller_of_record: nimbus,
            product_ids: vec!["p-0002-firewall-appliance".to_string()],
            offer_set_id: None,
            agreement_proposal_id: "ap-0003-firewall-hourly".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("USAGE", "Usage based"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0003-firewall-hourly".to_string(),
        vec![
            validity_term("t-0003-validity", "P365D", year_ago, next_year),
            usage_based_term("t-0003-usage", "ProtectedHost", "Protected host", "Hosts", "0.40"),
        ],
    );

    // ---- l-0003-datafeed -------------------------------------------------
    let orion = seller("sp-orion", "Orion Data");
    state.listings.insert(
        "l-0003-datafeed".to_string(),
        Listing {
            listing_id: "l-0003-datafeed".to_string(),
            text: Localized {
                default: text(
                    "Orion Market Data Feed",
                    "End-of-day and intraday reference prices for listed equities.",
                    "The Orion Market Data Feed delivers end-of-day and intraday reference \
                     prices for listed equities through AWS Data Exchange.",
                ),
                translations: BTreeMap::new(),
            },
            logo_thumbnail_url: "https://example.com/logos/orion-feed.png".to_string(),
            integration_guide: None,
            categories: classified_under(&["data-products"]),
            highlights: vec!["Covers 40 exchanges".to_string()],
            pricing_models: vec![pricing_model("CONTRACT", "Contract")],
            pricing_units: vec![pricing_unit("DATA", "Data")],
            product_ids: vec!["p-0003-datafeed-equities".to_string()],
            offer_ids: vec!["o-0004-datafeed-contract".to_string()],
            publisher: orion.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "data@example.com",
                "+1-206-555-0103",
                "https://example.com/docs/orion-feed",
            ),
            seller_engagements: engagements("https://example.com/orion/contact"),
            use_cases: vec![use_case(
                "portfolio-valuation",
                "Portfolio valuation",
                "Value portfolios against end-of-day reference prices.",
            )],
            review_summary: reviews("3.8", 11),
        },
    );
    state.products.insert(
        "p-0003-datafeed-equities".to_string(),
        Product {
            product_id: "p-0003-datafeed-equities".to_string(),
            listing_id: "l-0003-datafeed".to_string(),
            text: Localized {
                default: text(
                    "Orion Equities Reference Prices",
                    "Reference price data set for listed equities.",
                    "A daily data set of reference prices for listed equities, delivered as \
                     Parquet through AWS Data Exchange.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: orion.clone(),
            logo_thumbnail_url: "https://example.com/logos/orion-feed.png".to_string(),
            categories: classified_under(&["data-products"]),
            highlights: vec!["Parquet and CSV".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "data@example.com",
                "+1-206-555-0103",
                "https://example.com/docs/orion-feed",
            ),
            seller_engagements: engagements("https://example.com/orion/contact"),
            fulfillment_options: vec![data_exchange_option(
                "fo-0006-datafeed-dataset",
                "arn:aws:dataexchange:us-east-1::data-sets/orion-equities-reference-prices",
                "Daily reference prices for listed equities, as Parquet and CSV.",
            )],
        },
    );
    state.offers.insert(
        "o-0004-datafeed-contract".to_string(),
        Offer {
            offer_id: "o-0004-datafeed-contract".to_string(),
            offer_name: Some("Orion Market Data Feed — private pricing".to_string()),
            name_translations: BTreeMap::new(),
            // Already past: this is the catalog's EXPIRED purchase option.
            expiration_time: Some(month_ago),
            available_from_time: Some(year_ago),
            seller_of_record: orion,
            product_ids: vec!["p-0003-datafeed-equities".to_string()],
            offer_set_id: None,
            agreement_proposal_id: "ap-0004-datafeed-contract".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("CONTRACT", "Contract"),
            visibility_scope: VISIBILITY_PRIVATE.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0004-datafeed-contract".to_string(),
        vec![
            validity_term("t-0004-validity", "P365D", year_ago, month_ago),
            legal_term(
                "t-0004-legal",
                "StandardDsa",
                "https://example.com/legal/standard-dsa",
            ),
        ],
    );

    // ---- l-0004-mlkit ----------------------------------------------------
    let zephyr = seller("sp-zephyr", "Zephyr Labs");
    state.listings.insert(
        "l-0004-mlkit".to_string(),
        Listing {
            listing_id: "l-0004-mlkit".to_string(),
            text: Localized {
                default: text(
                    "Zephyr Vision ML Toolkit",
                    "Pre-trained vision and language models packaged for SageMaker.",
                    "The Zephyr Vision ML Toolkit packages pre-trained image and text models \
                     for SageMaker endpoints and container runtimes.",
                ),
                translations: BTreeMap::new(),
            },
            logo_thumbnail_url: "https://example.com/logos/zephyr-mlkit.png".to_string(),
            integration_guide: Some("https://example.com/docs/zephyr/integrate".to_string()),
            categories: classified_under(&["machine-learning"]),
            highlights: vec![
                "12 pre-trained models".to_string(),
                "Runs on SageMaker".to_string(),
            ],
            pricing_models: vec![
                pricing_model("USAGE", "Usage based"),
                pricing_model("FREE", "Free"),
            ],
            pricing_units: vec![pricing_unit("REQUESTS", "Requests")],
            product_ids: vec![
                "p-0004-mlkit-vision".to_string(),
                "p-0005-mlkit-nlp".to_string(),
            ],
            offer_ids: vec!["o-0005-mlkit-usage".to_string()],
            publisher: zephyr.clone(),
            promotional_media: vec![
                promotional_image(
                    "Defect detection",
                    "https://example.com/media/zephyr/defects.png",
                    "Detected defects on a production line frame.",
                ),
                promotional_video(
                    "Toolkit walkthrough",
                    "https://example.com/media/zephyr/walkthrough",
                ),
            ],
            resources: support_resources(
                "ml@example.com",
                "+1-206-555-0104",
                "https://example.com/docs/zephyr",
            ),
            seller_engagements: engagements("https://example.com/zephyr/contact"),
            use_cases: vec![
                use_case(
                    "document-understanding",
                    "Document understanding",
                    "Extract structure from scanned documents.",
                ),
                use_case(
                    "visual-inspection",
                    "Visual inspection",
                    "Spot defects on a production line from camera frames.",
                ),
            ],
            review_summary: reviews("4.9", 240),
        },
    );
    state.products.insert(
        "p-0004-mlkit-vision".to_string(),
        Product {
            product_id: "p-0004-mlkit-vision".to_string(),
            listing_id: "l-0004-mlkit".to_string(),
            text: Localized {
                default: text(
                    "Zephyr Vision Models",
                    "Image classification and detection models for SageMaker.",
                    "Zephyr Vision Models is a family of image classification, detection and \
                     segmentation models deployable to SageMaker endpoints.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: zephyr.clone(),
            logo_thumbnail_url: "https://example.com/logos/zephyr-mlkit.png".to_string(),
            categories: classified_under(&["machine-learning"]),
            highlights: vec!["GPU and CPU variants".to_string()],
            promotional_media: vec![promotional_video(
                "Vision models in SageMaker",
                "https://example.com/media/zephyr/vision",
            )],
            resources: support_resources(
                "ml@example.com",
                "+1-206-555-0104",
                "https://example.com/docs/zephyr",
            ),
            seller_engagements: engagements("https://example.com/zephyr/contact"),
            fulfillment_options: vec![
                sagemaker_model_option(
                    "fo-0007-mlkit-vision-model",
                    "3.1.0",
                    "Deploy the model package to a SageMaker endpoint and post images to it.",
                    &["image/jpeg", "image/png"],
                ),
                sagemaker_algorithm_option(
                    "fo-0008-mlkit-vision-algorithm",
                    "3.1.0",
                    "Fine-tune the detector on your own labelled frames with a SageMaker \
                     training job.",
                ),
            ],
        },
    );
    state.products.insert(
        "p-0005-mlkit-nlp".to_string(),
        Product {
            product_id: "p-0005-mlkit-nlp".to_string(),
            listing_id: "l-0004-mlkit".to_string(),
            text: Localized {
                default: text(
                    "Zephyr Language Models",
                    "Text classification and extraction models in a container.",
                    "Zephyr Language Models ships text classification, entity extraction and \
                     summarisation models as an OCI container.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: zephyr.clone(),
            logo_thumbnail_url: "https://example.com/logos/zephyr-mlkit.png".to_string(),
            categories: classified_under(&["machine-learning"]),
            highlights: vec!["Runs on EKS or ECS".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "ml@example.com",
                "+1-206-555-0104",
                "https://example.com/docs/zephyr",
            ),
            seller_engagements: engagements("https://example.com/zephyr/contact"),
            fulfillment_options: vec![container_option(
                "fo-0009-mlkit-nlp-container",
                "Zephyr Language Models",
                "3.1.0",
                "Pull the image from the registry and run it behind your own ingress.",
            )],
        },
    );
    state.offers.insert(
        "o-0005-mlkit-usage".to_string(),
        Offer {
            offer_id: "o-0005-mlkit-usage".to_string(),
            offer_name: Some("Zephyr Vision ML Toolkit — per request".to_string()),
            name_translations: BTreeMap::new(),
            expiration_time: Some(next_year),
            available_from_time: Some(year_ago),
            seller_of_record: zephyr,
            product_ids: vec![
                "p-0004-mlkit-vision".to_string(),
                "p-0005-mlkit-nlp".to_string(),
            ],
            offer_set_id: None,
            agreement_proposal_id: "ap-0005-mlkit-usage".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("USAGE", "Usage based"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0005-mlkit-usage".to_string(),
        vec![
            validity_term("t-0005-validity", "P365D", year_ago, next_year),
            usage_based_term("t-0005-usage", "Inference", "Inference request", "Requests", "0.002"),
            support_term("t-0005-support", "No refunds on consumed inference."),
        ],
    );

    // ---- l-0005-devtools -------------------------------------------------
    let helios = seller("sp-helios", "Helios Tools");
    state.listings.insert(
        "l-0005-devtools".to_string(),
        Listing {
            listing_id: "l-0005-devtools".to_string(),
            text: Localized {
                default: text(
                    "Helios DevOps Toolchain",
                    "Build, release and policy tooling for container platforms.",
                    "The Helios DevOps Toolchain bundles build caching, release promotion and \
                     policy checks for Kubernetes platforms, installed with Helm.",
                ),
                translations: BTreeMap::new(),
            },
            logo_thumbnail_url: "https://example.com/logos/helios-devops.png".to_string(),
            integration_guide: None,
            categories: classified_under(&["devops"]),
            highlights: vec!["Bring your own licence".to_string()],
            pricing_models: vec![pricing_model("BYOL", "Bring your own licence")],
            pricing_units: vec![pricing_unit("UNITS", "Units")],
            product_ids: vec!["p-0006-devtools-pipeline".to_string()],
            offer_ids: vec!["o-0006-devtools-byol".to_string()],
            publisher: helios.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "dev@example.com",
                "+1-206-555-0105",
                "https://example.com/docs/helios",
            ),
            seller_engagements: engagements("https://example.com/helios/contact"),
            use_cases: vec![use_case(
                "release-automation",
                "Release automation",
                "Promote builds between environments with policy gates.",
            )],
            review_summary: reviews("4.3", 77),
        },
    );
    state.products.insert(
        "p-0006-devtools-pipeline".to_string(),
        Product {
            product_id: "p-0006-devtools-pipeline".to_string(),
            listing_id: "l-0005-devtools".to_string(),
            text: Localized {
                default: text(
                    "Helios Pipeline Server",
                    "The Helios build and release server, as a Helm chart.",
                    "Helios Pipeline Server runs build and release pipelines inside your own \
                     Kubernetes cluster.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: helios.clone(),
            logo_thumbnail_url: "https://example.com/logos/helios-devops.png".to_string(),
            categories: classified_under(&["devops"]),
            highlights: vec!["Self hosted".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "dev@example.com",
                "+1-206-555-0105",
                "https://example.com/docs/helios",
            ),
            seller_engagements: engagements("https://example.com/helios/contact"),
            fulfillment_options: vec![
                helm_option(
                    "fo-0010-devtools-helm",
                    "Helios Pipeline Server",
                    "2.4.0",
                    "helm install helios oci://example.com/charts/helios-pipeline --version \
                     2.4.0",
                ),
                container_option(
                    "fo-0011-devtools-container",
                    "Helios Pipeline Server",
                    "2.4.0",
                    "Run the image directly when you do not install with Helm.",
                ),
                eks_add_on_option(
                    "fo-0012-devtools-eks-add-on",
                    "Helios policy gate",
                    "2.4.0",
                    "Enable the add-on on the cluster that runs your pipelines.",
                ),
            ],
        },
    );
    state.offers.insert(
        "o-0006-devtools-byol".to_string(),
        Offer {
            offer_id: "o-0006-devtools-byol".to_string(),
            offer_name: Some("Helios DevOps Toolchain — BYOL".to_string()),
            name_translations: BTreeMap::new(),
            expiration_time: None,
            available_from_time: Some(year_ago),
            seller_of_record: helios,
            product_ids: vec!["p-0006-devtools-pipeline".to_string()],
            offer_set_id: None,
            agreement_proposal_id: "ap-0006-devtools-byol".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("BYOL", "Bring your own licence"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0006-devtools-byol".to_string(),
        vec![legal_term(
            "t-0006-legal",
            "CustomEula",
            "https://example.com/legal/helios-eula",
        )],
    );

    // ---- l-0006-backup ---------------------------------------------------
    let vertex = seller("sp-vertex", "Vertex Storage");
    state.listings.insert(
        "l-0006-backup".to_string(),
        Listing {
            listing_id: "l-0006-backup".to_string(),
            text: Localized {
                default: text(
                    "Vertex Backup Analytics",
                    "Reporting and anomaly detection over backup estates.",
                    "Vertex Backup Analytics reports on backup coverage, restore readiness and \
                     anomalous change rates across an estate.",
                ),
                translations: BTreeMap::new(),
            },
            logo_thumbnail_url: "https://example.com/logos/vertex-backup.png".to_string(),
            integration_guide: None,
            categories: classified_under(&["storage"]),
            highlights: vec!["Coverage reporting".to_string()],
            pricing_models: vec![pricing_model("CONTRACT", "Contract")],
            pricing_units: vec![pricing_unit("TIERS", "Tiers")],
            product_ids: vec!["p-0007-backup-agent".to_string()],
            offer_ids: vec!["o-0007-backup-contract".to_string()],
            publisher: vertex.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "backup@example.com",
                "+1-206-555-0106",
                "https://example.com/docs/vertex",
            ),
            seller_engagements: engagements("https://example.com/vertex/contact"),
            use_cases: vec![use_case(
                "backup-reporting",
                "Backup reporting",
                "Report on backup coverage and restore readiness.",
            )],
            review_summary: reviews("4.0", 33),
        },
    );
    state.products.insert(
        "p-0007-backup-agent".to_string(),
        Product {
            product_id: "p-0007-backup-agent".to_string(),
            listing_id: "l-0006-backup".to_string(),
            text: Localized {
                default: text(
                    "Vertex Backup Collector",
                    "The hosted collector behind Vertex Backup Analytics.",
                    "The Vertex Backup Collector reads backup inventories and feeds the Vertex \
                     Backup Analytics reports.",
                ),
                translations: BTreeMap::new(),
            },
            manufacturer: vertex.clone(),
            logo_thumbnail_url: "https://example.com/logos/vertex-backup.png".to_string(),
            categories: classified_under(&["storage"]),
            highlights: vec!["Read-only access".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "backup@example.com",
                "+1-206-555-0106",
                "https://example.com/docs/vertex",
            ),
            seller_engagements: engagements("https://example.com/vertex/contact"),
            fulfillment_options: vec![
                saas_option(
                    "fo-0013-backup-saas",
                    "https://example.com/vertex/register",
                    "https://example.com/vertex/launch",
                    "DISABLED",
                    "Grant the collector read-only access to your backup inventories, then \
                     open the Vertex console.",
                    month_ago,
                ),
                professional_services_option("fo-0014-backup-onboarding"),
            ],
        },
    );
    state.offers.insert(
        "o-0007-backup-contract".to_string(),
        Offer {
            offer_id: "o-0007-backup-contract".to_string(),
            offer_name: Some("Vertex Backup Analytics — annual".to_string()),
            name_translations: BTreeMap::new(),
            expiration_time: Some(next_year),
            available_from_time: Some(month_ago),
            seller_of_record: vertex,
            product_ids: vec!["p-0007-backup-agent".to_string()],
            offer_set_id: None,
            agreement_proposal_id: "ap-0007-backup-contract".to_string(),
            replacement_agreement_id: None,
            pricing_model: pricing_model("CONTRACT", "Contract"),
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0007-backup-contract".to_string(),
        vec![
            validity_term("t-0007-validity", "P365D", month_ago, next_year),
            free_trial_term(
                "t-0007-free-trial",
                "P30D",
                "ProtectedTier",
                "Protected tier",
                "Tiers",
            ),
            recurring_payment_term("t-0007-recurring", "450.00"),
            // Renews for as long as the buyer keeps it: a renewal term with no
            // `maxRenewals` is "unlimited renewals".
            renewal_term("t-0007-renewal", None),
        ],
    );

    seed_breadth(&mut state, now);

    state
}

// ---------------------------------------------------------------------------
// The catalog's breadth: the listings that make paging, the whole fulfillment
// union and the category taxonomy reachable.
// ---------------------------------------------------------------------------

/// One row of the catalog's breadth: a single-product listing whose listing, product,
/// offer, offer terms and (for every third row) offer set are all derived from this row.
///
/// Why the catalog holds more listings than the six written out above:
///
/// * `SearchListings` is `@paginated` with `maxResults` defaulting to **25** and returns
///   `nextToken` "[i]f nextToken is returned, there are more results available"
///   (`smithy_query(query="operation",operation="SearchListings")`), so a catalog of fewer
///   than 26 listings can never truncate a default-sized page and a paginating client would
///   never be handed the token that fetches a second page.
/// * `FulfillmentOption` is a union with one member per `FulfillmentOptionType`, so every
///   type the model declares has to be reachable from some product — otherwise a client
///   following one fulfillment path finds no product that takes it.
/// * The category taxonomy is a tree ([`CATEGORY_TAXONOMY`]), so listings have to sit at its
///   subcategories — at every level of them — for a `CATEGORY` facet value to have a
///   `parent` to report, and for a parent to have one of its own.
struct BreadthRow {
    /// The slug every id of this row's records is built from.
    slug: &'static str,
    name: &'static str,
    short_description: &'static str,
    /// The taxonomy categories the listing is classified under; each one's ancestors come
    /// with it through [`classified_under`]. A record may sit at more than one category
    /// (`CategoryList` holds up to 30), and then carries every one of their paths.
    categories: &'static [&'static str],
    /// The `FulfillmentOptionType` its single product is fulfilled through, which also
    /// decides the product's `deployedOnAws` ([`Product::deployed_on_aws`]).
    fulfillment: &'static str,
}

/// The catalog's breadth rows, one listing each.
///
/// The set spans every top-level category of [`CATEGORY_TAXONOMY`], categories at each of
/// its three levels, every `FulfillmentOptionType` the model declares and — through the
/// deployment each fulfillment implies — all three `DeployedOnAwsStatus` values; the records
/// themselves are this emulator's own seed data.
const BREADTH_ROWS: &[BreadthRow] = &[
    BreadthRow {
        slug: "aurora-linux-hardened",
        name: "Aurora Hardened Linux",
        short_description: "Hardened Linux images with CIS benchmark profiles applied.",
        categories: &["operating-systems"],
        fulfillment: "AMAZON_MACHINE_IMAGE",
    },
    BreadthRow {
        slug: "borealis-market-data",
        name: "Borealis Market Data Feed",
        short_description: "End-of-day and intraday reference prices for listed equities.",
        // Two leaves of one subtree: the listing carries both paths, so its categories run
        // three levels deep and name two sibling subcategories.
        categories: &["market-data", "reference-data"],
        fulfillment: "DATA_EXCHANGE",
    },
    BreadthRow {
        slug: "cirrus-waf",
        name: "Cirrus Web Application Firewall",
        short_description: "Managed web application firewall rules for edge and VPC traffic.",
        categories: &["security"],
        fulfillment: "CLOUDFORMATION_TEMPLATE",
    },
    BreadthRow {
        slug: "delta-vision-labeler",
        name: "Delta Vision Labeler",
        short_description: "Image classification models for industrial inspection.",
        categories: &["computer-vision"],
        fulfillment: "SAGEMAKER_MODEL",
    },
    BreadthRow {
        slug: "equinox-fleet-telemetry",
        name: "Equinox Fleet Telemetry",
        short_description: "Telemetry ingestion and alerting for connected equipment.",
        categories: &["iot-analytics"],
        fulfillment: "SAAS",
    },
    BreadthRow {
        slug: "aurora-object-vault",
        name: "Aurora Object Vault",
        short_description: "Immutable object storage with compliance retention.",
        categories: &["storage"],
        fulfillment: "CONTAINER",
    },
    BreadthRow {
        slug: "borealis-health-atlas",
        name: "Borealis Health Atlas",
        short_description: "De-identified claims and prescription records for research.",
        categories: &["healthcare-life-sciences-data"],
        fulfillment: "DATA_EXCHANGE",
    },
    BreadthRow {
        slug: "cirrus-secrets-broker",
        name: "Cirrus Secrets Broker",
        short_description: "Short-lived credential brokering for cluster workloads.",
        categories: &["security"],
        fulfillment: "HELM",
    },
    BreadthRow {
        slug: "delta-speech-studio",
        name: "Delta Speech Studio",
        short_description: "Speech transcription and diarisation for contact centres.",
        categories: &["speech-recognition"],
        fulfillment: "SAGEMAKER_ALGORITHM",
    },
    BreadthRow {
        slug: "equinox-grid-forecast",
        name: "Equinox Grid Forecast",
        short_description: "Load and generation forecasting for energy networks.",
        categories: &["energy"],
        fulfillment: "API",
    },
    BreadthRow {
        slug: "aurora-build-cache",
        name: "Aurora Build Cache",
        short_description: "Distributed build caching for large monorepos.",
        categories: &["application-development"],
        fulfillment: "EKS_ADD_ON",
    },
    BreadthRow {
        slug: "borealis-climate-grid",
        name: "Borealis Climate Grid",
        short_description: "Gridded climate observations and reanalysis series.",
        categories: &["environmental-data"],
        fulfillment: "DATA_EXCHANGE",
    },
    BreadthRow {
        slug: "cirrus-migration-studio",
        name: "Cirrus Migration Studio",
        short_description: "Assessment and replatforming services for legacy estates.",
        categories: &["migration"],
        fulfillment: "PROFESSIONAL_SERVICES",
    },
    BreadthRow {
        slug: "delta-forms-reader",
        name: "Delta Forms Reader",
        short_description: "Document understanding for structured and semi-structured forms.",
        categories: &["natural-language-processing"],
        fulfillment: "SAGEMAKER_MODEL",
    },
    BreadthRow {
        slug: "equinox-retail-planner",
        name: "Equinox Retail Planner",
        short_description: "Assortment and replenishment planning for retailers.",
        categories: &["retail"],
        fulfillment: "SAAS",
    },
    BreadthRow {
        slug: "aurora-golden-images",
        name: "Aurora Golden Images",
        short_description: "Image pipeline components for compliant base images.",
        categories: &["infrastructure-as-code"],
        fulfillment: "EC2_IMAGE_BUILDER_COMPONENT",
    },
    BreadthRow {
        slug: "borealis-spectrum-usage",
        name: "Borealis Spectrum Usage",
        short_description: "Mobile network coverage and spectrum usage records.",
        categories: &["telecommunications-data"],
        fulfillment: "DATA_EXCHANGE",
    },
    BreadthRow {
        slug: "cirrus-observability",
        name: "Cirrus Observability Stack",
        short_description: "Metrics, traces and log correlation for microservices.",
        categories: &["monitoring"],
        fulfillment: "HELM",
    },
    BreadthRow {
        slug: "delta-crm-sync",
        name: "Delta CRM Sync",
        short_description: "Two-way synchronisation between CRM and billing systems.",
        categories: &["crm"],
        fulfillment: "SAAS",
    },
    BreadthRow {
        slug: "equinox-hpc-solver",
        name: "Equinox HPC Solver",
        short_description: "Finite element solver for structural simulation.",
        categories: &["high-performance-computing"],
        fulfillment: "AMAZON_MACHINE_IMAGE",
    },
    BreadthRow {
        slug: "aurora-training-labs",
        name: "Aurora Training Labs",
        short_description: "Instructor-led enablement for platform engineering teams.",
        categories: &["training"],
        fulfillment: "PROFESSIONAL_SERVICES",
    },
    BreadthRow {
        slug: "borealis-data-prep",
        name: "Borealis Data Prep Studio",
        short_description: "Profiling, cleansing and join authoring for analytics data sets.",
        // Both sibling leaves of Data Analytics, so the listing carries the taxonomy's
        // deepest path twice over: four categories across three levels.
        categories: &["data-integration", "data-preparation"],
        fulfillment: "SAAS",
    },
    BreadthRow {
        slug: "cirrus-query-lens",
        name: "Cirrus Query Lens",
        short_description: "Interactive dashboards over warehouse and lakehouse tables.",
        categories: &["data-visualization"],
        fulfillment: "CONTAINER",
    },
];

/// The sellers the breadth rows are published by, taken in turn.
const BREADTH_SELLERS: &[(&str, &str)] = &[
    ("sp-aurora", "Aurora Systems"),
    ("sp-borealis", "Borealis Data"),
    ("sp-cirrus", "Cirrus Security"),
    ("sp-delta-labs", "Delta Labs"),
    ("sp-equinox", "Equinox Industrial"),
];

/// Every pricing model with the name the catalog displays it by.
///
/// Source: the `PricingModelType` enum
/// (`smithy_query(query="shape",shape="PricingModelType")`), generated as
/// `PRICING_MODEL_TYPE_VALUES`; the display names are the catalog's own wording and are the
/// same ones the hand-written records above use.
const PRICING_MODEL_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("USAGE", "Usage based"),
    ("CONTRACT", "Contract"),
    ("BYOL", "Bring your own licence"),
    ("FREE", "Free"),
];

/// Every pricing unit with the name the catalog displays it by.
///
/// Source: the `PricingUnitType` enum
/// (`smithy_query(query="shape",shape="PricingUnitType")`), generated as
/// `PRICING_UNIT_TYPE_VALUES`.
const PRICING_UNIT_DISPLAY_NAMES: &[(&str, &str)] = &[
    ("USERS", "Users"),
    ("HOSTS", "Hosts"),
    ("BANDWIDTH", "Bandwidth"),
    ("DATA", "Data"),
    ("TIERS", "Tiers"),
    ("REQUESTS", "Requests"),
    ("UNITS", "Units"),
];

/// The review summaries the breadth rows carry, taken in turn: `(averageRating,
/// totalReviews)`. They span the whole 0.0–5.0 rating range the
/// `MIN`/`MAX_AVERAGE_CUSTOMER_RATING` filters bound, so a rating filter and the
/// `AVERAGE_CUSTOMER_RATING` facet select some listings and not others.
const BREADTH_REVIEWS: &[(&str, i64)] = &[
    ("4.8", 310),
    ("4.5", 164),
    ("4.1", 88),
    ("3.7", 42),
    ("3.2", 19),
    ("2.6", 11),
    ("4.9", 7),
];

/// A fulfillment option of `option_type` for a breadth row's product.
///
/// One arm per `FulfillmentOptionType`, each building the `FulfillmentOption` UNION MEMBER
/// that type belongs to, so a client that reads a product's fulfillment option type and
/// then deserialises that type's variant finds it.
fn fulfillment_option_for_type(
    option_type: &str,
    id: &str,
    name: &str,
    slug: &str,
    available_from: i64,
) -> FulfillmentOptionRecord {
    match option_type {
        "SAAS" => saas_option(
            id,
            &format!("https://example.com/{slug}/register"),
            &format!("https://example.com/{slug}/launch"),
            "ENABLED",
            &format!("Subscribe, then open the {name} console."),
            available_from,
        ),
        "API" => api_option(
            id,
            &format!("Call the {name} API with the key issued at registration."),
            vec![aws_service(
                "APIGATEWAY",
                "Amazon API Gateway",
                &format!("Fronts the {name} API."),
            )],
        ),
        "AMAZON_MACHINE_IMAGE" => ami_option(
            id,
            name,
            "1.0.0",
            "x86_64",
            &format!("Launch the {name} image into a private subnet."),
            "m6i.large",
            available_from,
        ),
        "CLOUDFORMATION_TEMPLATE" => cloudformation_option(
            id,
            name,
            "1.0.0",
            &format!("Create the {name} stack from the template."),
            &format!("{name}, as an AWS CloudFormation template."),
            available_from,
        ),
        "CONTAINER" => container_option(
            id,
            name,
            "1.0.0",
            &format!("Run the {name} image on Amazon ECS or Amazon EKS."),
        ),
        "HELM" => helm_option(
            id,
            name,
            "1.0.0",
            &format!("helm install {slug} oci://example.com/charts/{slug} --version 1.0.0"),
        ),
        "EKS_ADD_ON" => eks_add_on_option(
            id,
            name,
            "1.0.0",
            &format!("Enable the {name} add-on on your cluster."),
        ),
        "EC2_IMAGE_BUILDER_COMPONENT" => ec2_image_builder_option(
            id,
            name,
            "1.0.0",
            &format!("Add the {name} component to an Image Builder recipe."),
        ),
        "DATA_EXCHANGE" => data_exchange_option(
            id,
            &format!("arn:aws:dataexchange:us-east-1::data-sets/{slug}"),
            &format!("The {name} data set."),
        ),
        "SAGEMAKER_ALGORITHM" => sagemaker_algorithm_option(
            id,
            "1.0.0",
            &format!("Train {name} from an Amazon SageMaker notebook."),
        ),
        "SAGEMAKER_MODEL" => sagemaker_model_option(
            id,
            "1.0.0",
            &format!("Deploy {name} to an Amazon SageMaker endpoint."),
            &["application/json", "image/png"],
        ),
        "PROFESSIONAL_SERVICES" => professional_services_option(id),
        // A `FulfillmentOptionType` the model GAINS later has no builder here, and seeding
        // it as another type's union member would make the catalog claim a fulfillment the
        // row does not have — so it fails loudly at seed time instead.
        other => unreachable!(
            "fulfillment option type {other} has no seed builder; add an arm for its \
             FulfillmentOption union member"
        ),
    }
}

/// Seed one listing, product, offer and set of offer terms per [`BREADTH_ROWS`] row.
///
/// Ids are built from the row's slug in their own `0101..` numeric block, so no generated id
/// can collide with the hand-written records above, and the whole block is derived from the
/// row rather than written out: what each row varies is what the catalog needs to span.
fn seed_breadth(state: &mut ComAmazonawsMarketplacediscoveryAwsmarketplacediscoveryState, now: i64) {
    let month_ago = now - 30 * DAY;
    let next_year = now + 365 * DAY;

    for (index, row) in BREADTH_ROWS.iter().enumerate() {
        let sequence = 101 + index;
        let slug = row.slug;
        let listing_id = format!("l-{sequence:04}-{slug}");
        let product_id = format!("p-{sequence:04}-{slug}");
        let offer_id = format!("o-{sequence:04}-{slug}");
        let option_id = format!("fo-{sequence:04}-{slug}");

        let (seller_profile_id, seller_name) = BREADTH_SELLERS[index % BREADTH_SELLERS.len()];
        let publisher = seller(seller_profile_id, seller_name);
        let (model_type, model_name) =
            PRICING_MODEL_DISPLAY_NAMES[index % PRICING_MODEL_DISPLAY_NAMES.len()];
        let (unit_type, unit_name) =
            PRICING_UNIT_DISPLAY_NAMES[index % PRICING_UNIT_DISPLAY_NAMES.len()];
        let (rating, total_reviews) = BREADTH_REVIEWS[index % BREADTH_REVIEWS.len()];

        let categories = classified_under(row.categories);
        // The row's own deepest category, which is the one its prose names: the expanded
        // list also holds the ancestors that came with it.
        let category_name = row
            .categories
            .iter()
            .next_back()
            .map_or_else(String::new, |leaf| taxonomy_category(leaf).display_name);
        let fulfillment_name = fulfillment_option_display_name(row.fulfillment);
        let long_description = format!(
            "{} {} is listed under {category_name} and fulfilled as {fulfillment_name}.",
            row.short_description, row.name
        );
        let listing_text = Localized {
            default: text(row.name, row.short_description, &long_description),
            translations: BTreeMap::new(),
        };
        let logo = format!("https://example.com/logos/{slug}.png");
        let docs = format!("https://example.com/docs/{slug}");
        let support_email = format!("support@{slug}.example.com");
        // A telephone number in the `555-01xx` range reserved for fictional use, one per
        // row and clear of the numbers the records above take, so the `PHONE_NUMBER`
        // resource of two rows is never the same number. BREADTH_ROWS is far shorter than
        // the 90 numbers left in the range.
        let support_phone = format!("+1-206-555-{:04}", 110 + index);
        let contact = format!("https://example.com/{slug}/contact");

        // A badge is not seeded: `listing_badges` derives each one from the records it
        // restates, so this row's deployment status and pricing model decide them.

        state.listings.insert(
            listing_id.clone(),
            Listing {
                listing_id: listing_id.clone(),
                text: listing_text.clone(),
                logo_thumbnail_url: logo.clone(),
                integration_guide: None,
                categories: categories.clone(),
                highlights: vec![
                    format!("Fulfilled as {fulfillment_name}"),
                    format!("Listed under {category_name}"),
                ],
                pricing_models: vec![pricing_model(model_type, model_name)],
                pricing_units: vec![pricing_unit(unit_type, unit_name)],
                product_ids: vec![product_id.clone()],
                offer_ids: vec![offer_id.clone()],
                publisher: publisher.clone(),
                promotional_media: vec![promotional_image(
                    row.name,
                    &format!("https://example.com/media/{slug}/overview.png"),
                    row.short_description,
                )],
                resources: support_resources(&support_email, &support_phone, &docs),
                seller_engagements: engagements(&contact),
                use_cases: vec![use_case(
                    slug,
                    row.name,
                    &format!(
                        "{} Listed under {category_name}.",
                        row.short_description
                    ),
                )],
                review_summary: reviews(rating, total_reviews),
            },
        );

        state.products.insert(
            product_id.clone(),
            Product {
                product_id: product_id.clone(),
                listing_id: listing_id.clone(),
                text: listing_text,
                manufacturer: publisher.clone(),
                logo_thumbnail_url: logo,
                categories,
                highlights: vec![format!("Fulfilled as {fulfillment_name}")],
                promotional_media: Vec::new(),
                resources: support_resources(&support_email, &support_phone, &docs),
                seller_engagements: engagements(&contact),
                fulfillment_options: vec![fulfillment_option_for_type(
                    row.fulfillment,
                    &option_id,
                    row.name,
                    slug,
                    month_ago,
                )],
            },
        );

        // Every third row's offer belongs to an offer set, so `OFFERSET` purchase options
        // are reachable for products across the catalog and not only for the one
        // hand-written set above.
        let offer_set_id = (index % 3 == 2).then(|| format!("os-{sequence:04}-{slug}"));
        state.offers.insert(
            offer_id.clone(),
            Offer {
                offer_id: offer_id.clone(),
                offer_name: Some(format!("{} — {model_name}", row.name)),
                name_translations: BTreeMap::new(),
                expiration_time: Some(next_year),
                available_from_time: Some(month_ago),
                seller_of_record: publisher.clone(),
                product_ids: vec![product_id],
                offer_set_id: offer_set_id.clone(),
                agreement_proposal_id: format!("ap-{sequence:04}-{slug}"),
                replacement_agreement_id: None,
                pricing_model: pricing_model(model_type, model_name),
                visibility_scope: VISIBILITY_PUBLIC.to_string(),
            },
        );

        let mut terms = vec![validity_term(
            &format!("t-{sequence:04}-validity"),
            "P365D",
            month_ago,
            next_year,
        )];
        match model_type {
            "USAGE" => terms.push(usage_based_term(
                &format!("t-{sequence:04}-usage"),
                unit_type,
                unit_name,
                unit_name,
                "0.05",
            )),
            "CONTRACT" => {
                terms.push(recurring_payment_term(
                    &format!("t-{sequence:04}-recurring"),
                    "900.00",
                ));
                // A contract renews at the end of each agreement period, which is what
                // this offer's AUTO_RENEW badge restates.
                terms.push(renewal_term(&format!("t-{sequence:04}-renewal"), Some(2)));
            }
            // BYOL and FREE carry no payment term: the buyer pays the seller directly, or
            // nothing at all.
            _ => {}
        }
        terms.push(legal_term(
            &format!("t-{sequence:04}-legal"),
            "StandardEula",
            "https://example.com/legal/standard-eula",
        ));
        state.offer_terms.insert(offer_id.clone(), terms);

        if let Some(offer_set_id) = offer_set_id {
            state.offer_sets.insert(
                offer_set_id.clone(),
                OfferSet {
                    offer_set_id,
                    offer_set_name: Some(format!("{} purchase options", row.name)),
                    available_from_time: Some(month_ago),
                    expiration_time: Some(next_year),
                    buyer_notes: None,
                    seller_of_record: publisher,
                    offer_ids: vec![offer_id],
                    visibility_scope: VISIBILITY_PUBLIC.to_string(),
                },
            );
        }
    }
}
