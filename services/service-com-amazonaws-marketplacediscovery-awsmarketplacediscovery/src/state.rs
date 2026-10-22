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
//!   [`matches_every_search_filter`]).

use std::collections::BTreeMap;

use crate::ops::types::{
    AmazonMachineImageEbsVolume, AmazonMachineImageFulfillmentOption,
    AmazonMachineImageOperatingSystem, AmazonMachineImageRecommendation,
    AmazonMachineImageSecurityGroup, ApiFulfillmentOption, AwsSupportedService, Category,
    CloudFormationFulfillmentOption, ContainerFulfillmentOption, ContainerOperatingSystem,
    DataArtifact, DataExchangeFulfillmentOption, DocumentItem,
    Ec2ImageBuilderComponentFulfillmentOption, EksAddOnFulfillmentOption,
    EksAddOnOperatingSystem, FulfillmentOption, FulfillmentOptionSummary, HelmFulfillmentOption,
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
    pub badges: Vec<ListingBadge>,
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
    pub deployed_on_aws: String,
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
    pub badges: Vec<PurchaseOptionBadge>,
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
    /// `searchText` matches a listing whose name or short description contains it, compared
    /// case-insensitively, and an absent or empty `searchText` matches every listing;
    /// `filters` then narrow the result per [`matches_every_search_filter`]. `Err` is that
    /// function's refusal message.
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
        let needle = search_text.trim().to_lowercase();
        let mut matches: Vec<&Listing> = Vec::new();
        for listing in self.listings.values() {
            if !needle.is_empty() && !search_haystack(listing, preferred).contains(&needle) {
                continue;
            }
            if !matches_every_search_filter(self, listing, filters)? {
                continue;
            }
            matches.push(listing);
        }
        Ok(matches)
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
            badges: listing.badges.clone(),
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
                    badges: Some(offer.badges.clone()),
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
                    badges: Some(Vec::new()),
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
/// id nor the catalog it was looked up in. `resource` is therefore the resource KIND
/// (`"Listing"`, `"Offer"`, `"Product"`), never a value out of the request.
pub fn not_found_message(resource: &str) -> String {
    format!("{resource} not found")
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
// Searching the catalog: what `SearchListings` and `SearchFacets` share.
// ---------------------------------------------------------------------------

/// The text a `searchText` is matched against: a listing's name and its short description,
/// in the locale its content is returned in, lowercased.
pub fn search_haystack(listing: &Listing, preferred: &'static str) -> String {
    let locale = listing.text.content_locale(preferred);
    let text = listing.text.text(locale);
    format!("{} {}", text.name, text.short_description).to_lowercase()
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
                    .is_some_and(|product| values.contains(&product.deployed_on_aws))
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
// The seeded catalog.
// ---------------------------------------------------------------------------

fn seller(profile_id: &str, display_name: &str) -> SellerInformation {
    SellerInformation {
        seller_profile_id: profile_id.to_string(),
        display_name: display_name.to_string(),
    }
}

fn category(category_id: &str, display_name: &str) -> Category {
    Category {
        category_id: category_id.to_string(),
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

fn listing_badge(badge_type: &str, display_name: &str) -> ListingBadge {
    ListingBadge {
        display_name: display_name.to_string(),
        badge_type: badge_type.to_string(),
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

fn support_resources(support_email: &str, docs_url: &str) -> Vec<Resource> {
    vec![
        Resource {
            resource_type: "MANUFACTURER_SUPPORT".to_string(),
            content_type: "EMAIL".to_string(),
            value: support_email.to_string(),
            display_name: Some("Support".to_string()),
        },
        Resource {
            resource_type: "MANUFACTURER_INSTRUCTIONS".to_string(),
            content_type: "LINK".to_string(),
            value: docs_url.to_string(),
            display_name: Some("Documentation".to_string()),
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

fn recurring_payment_term(id: &str, price: &str) -> OfferTerm {
    OfferTerm::recurringPaymentTerm(RecurringPaymentTerm {
        id: id.to_string(),
        r#type: "RecurringPaymentTerm".to_string(),
        currency_code: "USD".to_string(),
        billing_period: "Monthly".to_string(),
        price: price.to_string(),
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
            categories: vec![
                category("business-applications", "Business Applications"),
                category("data-products", "Data Products"),
            ],
            highlights: vec![
                "Connects to 40 data sources".to_string(),
                "Row-level access control".to_string(),
            ],
            badges: vec![listing_badge("FREE_TRIAL", "Free trial")],
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
            deployed_on_aws: "NOT_APPLICABLE".to_string(),
            logo_thumbnail_url: "https://example.com/logos/acme-analytics.png".to_string(),
            categories: vec![category("business-applications", "Business Applications")],
            highlights: vec!["Hosted by the seller".to_string()],
            promotional_media: vec![promotional_image(
                "Query engine",
                "https://example.com/media/acme-analytics/engine.png",
                "The modelling layer of Acme Analytics Core.",
            )],
            resources: support_resources(
                "support@example.com",
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
            badges: vec![purchase_option_badge("AUTO_RENEW", "Auto renew")],
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0001-analytics-annual".to_string(),
        vec![
            validity_term("t-0001-validity", "P365D", year_ago, next_year),
            recurring_payment_term("t-0001-recurring", "1200.00"),
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
            badges: vec![purchase_option_badge("FUTURE_DATED", "Future dated")],
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
            categories: vec![category("infrastructure-software", "Infrastructure Software")],
            highlights: vec!["Deep packet inspection".to_string()],
            badges: vec![
                listing_badge("AWS_FREE_TIER", "AWS Free Tier"),
                listing_badge("QUICK_LAUNCH", "Quick Launch"),
            ],
            pricing_models: vec![pricing_model("USAGE", "Usage based")],
            pricing_units: vec![pricing_unit("HOSTS", "Hosts")],
            product_ids: vec!["p-0002-firewall-appliance".to_string()],
            offer_ids: vec!["o-0003-firewall-hourly".to_string()],
            publisher: nimbus.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "help@example.com",
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
            deployed_on_aws: "DEPLOYED".to_string(),
            logo_thumbnail_url: "https://example.com/logos/nimbus-firewall.png".to_string(),
            categories: vec![category("infrastructure-software", "Infrastructure Software")],
            highlights: vec!["Runs in your account".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources(
                "help@example.com",
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
            badges: Vec::new(),
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
            categories: vec![category("data-products", "Data Products")],
            highlights: vec!["Covers 40 exchanges".to_string()],
            badges: Vec::new(),
            pricing_models: vec![pricing_model("CONTRACT", "Contract")],
            pricing_units: vec![pricing_unit("DATA", "Data")],
            product_ids: vec!["p-0003-datafeed-equities".to_string()],
            offer_ids: vec!["o-0004-datafeed-contract".to_string()],
            publisher: orion.clone(),
            promotional_media: Vec::new(),
            resources: support_resources(
                "data@example.com",
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
            deployed_on_aws: "NOT_APPLICABLE".to_string(),
            logo_thumbnail_url: "https://example.com/logos/orion-feed.png".to_string(),
            categories: vec![category("data-products", "Data Products")],
            highlights: vec!["Parquet and CSV".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources("data@example.com", "https://example.com/docs/orion-feed"),
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
            badges: vec![purchase_option_badge("PRIVATE_PRICING", "Private pricing")],
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
            categories: vec![category("machine-learning", "Machine Learning")],
            highlights: vec![
                "12 pre-trained models".to_string(),
                "Runs on SageMaker".to_string(),
            ],
            badges: vec![
                listing_badge("DEPLOYED_ON_AWS", "Deployed on AWS"),
                listing_badge("MULTI_PRODUCT", "Multiple products"),
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
            resources: support_resources("ml@example.com", "https://example.com/docs/zephyr"),
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
            deployed_on_aws: "DEPLOYED".to_string(),
            logo_thumbnail_url: "https://example.com/logos/zephyr-mlkit.png".to_string(),
            categories: vec![category("machine-learning", "Machine Learning")],
            highlights: vec!["GPU and CPU variants".to_string()],
            promotional_media: vec![promotional_video(
                "Vision models in SageMaker",
                "https://example.com/media/zephyr/vision",
            )],
            resources: support_resources("ml@example.com", "https://example.com/docs/zephyr"),
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
            deployed_on_aws: "DEPLOYED".to_string(),
            logo_thumbnail_url: "https://example.com/logos/zephyr-mlkit.png".to_string(),
            categories: vec![category("machine-learning", "Machine Learning")],
            highlights: vec!["Runs on EKS or ECS".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources("ml@example.com", "https://example.com/docs/zephyr"),
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
            badges: Vec::new(),
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
            categories: vec![category("devops", "DevOps")],
            highlights: vec!["Bring your own licence".to_string()],
            badges: Vec::new(),
            pricing_models: vec![pricing_model("BYOL", "Bring your own licence")],
            pricing_units: vec![pricing_unit("UNITS", "Units")],
            product_ids: vec!["p-0006-devtools-pipeline".to_string()],
            offer_ids: vec!["o-0006-devtools-byol".to_string()],
            publisher: helios.clone(),
            promotional_media: Vec::new(),
            resources: support_resources("dev@example.com", "https://example.com/docs/helios"),
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
            deployed_on_aws: "NOT_DEPLOYED".to_string(),
            logo_thumbnail_url: "https://example.com/logos/helios-devops.png".to_string(),
            categories: vec![category("devops", "DevOps")],
            highlights: vec!["Self hosted".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources("dev@example.com", "https://example.com/docs/helios"),
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
            badges: Vec::new(),
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
            categories: vec![category("storage", "Storage")],
            highlights: vec!["Coverage reporting".to_string()],
            badges: vec![listing_badge("FREE_TRIAL", "Free trial")],
            pricing_models: vec![pricing_model("CONTRACT", "Contract")],
            pricing_units: vec![pricing_unit("TIERS", "Tiers")],
            product_ids: vec!["p-0007-backup-agent".to_string()],
            offer_ids: vec!["o-0007-backup-contract".to_string()],
            publisher: vertex.clone(),
            promotional_media: Vec::new(),
            resources: support_resources("backup@example.com", "https://example.com/docs/vertex"),
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
            deployed_on_aws: "NOT_APPLICABLE".to_string(),
            logo_thumbnail_url: "https://example.com/logos/vertex-backup.png".to_string(),
            categories: vec![category("storage", "Storage")],
            highlights: vec!["Read-only access".to_string()],
            promotional_media: Vec::new(),
            resources: support_resources("backup@example.com", "https://example.com/docs/vertex"),
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
            badges: vec![purchase_option_badge("AUTO_RENEW", "Auto renew")],
            visibility_scope: VISIBILITY_PUBLIC.to_string(),
        },
    );
    state.offer_terms.insert(
        "o-0007-backup-contract".to_string(),
        vec![
            validity_term("t-0007-validity", "P365D", month_ago, next_year),
            recurring_payment_term("t-0007-recurring", "450.00"),
        ],
    );

    state
}
