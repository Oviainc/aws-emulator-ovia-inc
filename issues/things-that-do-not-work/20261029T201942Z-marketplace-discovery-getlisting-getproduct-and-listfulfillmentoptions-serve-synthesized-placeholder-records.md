Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-listings-and-drill-into-listing
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-listings-and-drill-into-listing
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.associatedEntities[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.badges[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.categories[*].categoryId
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.categories[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.highlights[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.listingName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.logoThumbnailUrl
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.longDescription
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.promotionalMedia[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.publisher.displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.publisher.sellerProfileId
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.purchaseIndicators
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].contentType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].displayName
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].resourceType
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.resources[*].value
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.sellerEngagements[*]
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.shortDescription
Issue-Key: marketplace-discovery|GetListing|2xx->2xx|$.useCases[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.categories[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.highlights[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.logoThumbnailUrl
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.longDescription
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.manufacturer.displayName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.manufacturer.sellerProfileId
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.productName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.purchaseIndicators
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].contentType
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].displayName
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].resourceType
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.resources[*].value
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.sellerEngagements[*]
Issue-Key: marketplace-discovery|GetProduct|2xx->2xx|$.shortDescription
Issue-Key: marketplace-discovery|ListFulfillmentOptions|2xx->2xx|$.fulfillmentOptions[*]
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# GetListing, GetProduct and ListFulfillmentOptions serve synthesized placeholder records instead of the catalog's stored content

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetListing

## Description
This issue bundles 3 causes, all in marketplace-discovery, all three the detail views of one drill-down flow (search -> listing -> product -> fulfillment options). In each one the emulator answers 200 but populates the response from a generated template rather than from the record the requested id names, and the divergence is structural, not just textual: identifier FORMAT, list LENGTH, list ORDER, and which optional members are present all differ from what this API returns.

Cause 1 of 3: GetListing's detail members are synthesized boilerplate, its categoryIds are slugs instead of taxonomy identifiers, its highlights are generated sentences, and it reports a badge the listing has not earned.
AWS: Smithy: GetListing output requires associatedEntities (ListingAssociatedEntityList, element pairs ProductInformation and OfferInformation), badges (ListingBadgeList @length ..20), categories (CategoryList @length ..30) of Category{categoryId NonEmptyString @length(1..1024) required, displayName required - documented "machine-readable identifier of the category" / "human-readable name"}, highlights (HighlightList @length ..10), listingName, logoThumbnailUrl (URL @pattern ^https?://.*$), longDescription (@length ..32768), promotionalMedia, publisher, resources, sellerEngagements, shortDescription, useCases. The model does not fix the content, so recorded behavior: the response restates the stored listing - categoryId is an opaque taxonomy identifier (a UUID in the recording, not a lowercased form of displayName), the categories are returned in the listing's stored order rather than ancestors-first, highlights are the seller's own highlight sentences (three of them here, up to the modelled ten), listingName / shortDescription / longDescription / logoThumbnailUrl / promotionalMedia / resources (each with its own contentType, displayName and value) / sellerEngagements / useCases / purchaseIndicators all come from the record, the publisher is the listing's publisher, and badges is EMPTY when the listing earns no badge.
Emulator: 200, but: categoryId is a slug derived from the display name ('infrastructure-software', 'data-analytics', 'data-integration', 'data-preparation') instead of a taxonomy identifier, and the list is ordered ancestors-first rather than in stored order, so element-for-element both the id and the displayName differ; highlights is exactly two generated sentences of the form 'Fulfilled as <fulfillment option display name>' and 'Listed under <category display name>', so the listing's own highlights are gone and the third stored highlight is absent entirely; badges[0] is reported (a QUICK_LAUNCH / 'Quick Launch' badge) where the recording reports none; associatedEntities[0]'s product and offer carry template names; and listingName, shortDescription, longDescription, logoThumbnailUrl, promotionalMedia, publisher, resources, sellerEngagements, useCases and purchaseIndicators are all template text of the form 'https://example.com/logos/<slug>.png' and '<short description> <name> is listed under <category> and fulfilled as <fulfillment>.'
Why: GetListing is the detail view the whole discovery flow funnels into. A client that renders it shows generated filler; worse, a client that takes a returned categoryId and feeds it back as a CATEGORY search filter, or persists it, is handling an identifier whose shape this service never issues, and one that reads `badges` sees a capability (quick launch) the listing does not have.
Fix: the seeded catalog and its projections in src/state.rs - the breadth-row seeder that synthesizes highlights, descriptions and logo URLs, the slug-keyed taxonomy helper that mints categoryId and expands ancestors ahead of the stored category, and the badge derivation that awards QUICK_LAUNCH to every seeded SaaS row; GetListing's handler should then restate the record rather than a template. Traces: src/state.rs:3603-3680 (the BREADTH_ROWS loop: highlights built as format!("Fulfilled as {fulfillment_name}") / format!("Listed under {category_name}"), logo as format!("https://example.com/logos/{slug}.png"), long_description as a formatted sentence, categories from classified_under(row.categories)).
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#2 (GetListing, expected 200 / got 200; 33 diverging keys including $.categories[0..3].categoryId/displayName, $.highlights[0..2], $.badges[0] extra, $.associatedEntities[0], $.listingName, $.logoThumbnailUrl, $.longDescription, $.publisher.*, $.resources[*], $.sellerEngagements[*], $.useCases[0], $.purchaseIndicators).

Cause 2 of 3: GetProduct repeats the same synthesized record for the product resolved from that listing.
AWS: Smithy: GetProduct output requires productId, catalog, productName, manufacturer (SellerInformation), deployedOnAws, shortDescription, longDescription, logoThumbnailUrl, fulfillmentOptionSummaries, categories (CategoryList of Category), highlights (HighlightList @length ..10), promotionalMedia, resources, sellerEngagements, listingId. Recorded behavior: the product restates its stored record, with the same taxonomy-identifier categories in stored order and the seller's own highlight sentences, its own manufacturer, descriptions, logo URL, resources and seller engagements.
Emulator: 200 with the same substitutions as cause 1 - slug categoryIds in ancestors-first order, the two generated 'Fulfilled as ...' / 'Listed under ...' highlights in place of the stored ones (the remaining stored highlights simply absent), and template productName, manufacturer, shortDescription, longDescription, logoThumbnailUrl, resources, sellerEngagements and purchaseIndicators.
Why: a client that drills product-level detail after a listing gets a second, independently wrong copy of the same record, so cross-checking listing against product cannot detect it; and the same slug-shaped categoryId leaks here too.
Fix: same component as cause 1 (the src/state.rs seeder and the category/highlight projections GetProduct shares with GetListing).
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#3 (GetProduct, expected 200 / got 200; 27 diverging keys including $.categories[0..3], $.highlights[0..2], $.logoThumbnailUrl, $.longDescription, $.manufacturer.*, $.productName, $.resources[*], $.sellerEngagements[*], $.shortDescription, $.purchaseIndicators).

Cause 3 of 3: ListFulfillmentOptions' SaaS option carries optional members the record does not have and reports quick launch as enabled for every row.
AWS: Smithy: ListFulfillmentOptions output requires fulfillmentOptions (FulfillmentOptionsList); the SaaS variant SaasFulfillmentOption requires fulfillmentOptionId, fulfillmentOptionType, fulfillmentOptionDisplayName and quickLaunch (SaasQuickLaunchStatus), and makes fulfillmentUrl, usageInstructions, availableFromTime and launchUrl optional (launchUrl is documented as "distinct from fulfillmentUrl"). Recorded behavior: the SaaS option reports quickLaunch DISABLED for a product that does not support quick-launch deployment, and omits usageInstructions and launchUrl entirely when the record carries none; fulfillmentUrl is the seller's own registration URL. (The recording additionally carries a deploymentTemplates member on the SaaS option that the on-disk model's SaasFulfillmentOption does not declare - recorded behavior only; a fixer should treat only the modelled members above as the requirement.)
Emulator: 200 but the SaaS option reports quickLaunch ENABLED, and emits usageInstructions and launchUrl that the record does not have, while omitting the recording's deploymentTemplates; fulfillmentUrl is a template 'https://example.com/<slug>/register' URL.
Why: quickLaunch and the presence of launchUrl are what a client branches on to decide whether it can offer a one-click launch; reporting ENABLED with a fabricated launchUrl for a product whose record disables it makes the emulator exercise a code path AWS would never take, and it is the same over-eager derivation that mints the unearned QUICK_LAUNCH badge in cause 1.
Fix: ListFulfillmentOptions' projection of the seeded fulfillment options in src/state.rs (the per-row SaaS option the breadth seeder builds, and the quick-launch status it derives) - quickLaunch must reflect the stored record, and the optional members must be emitted only when the record carries them.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#4 (ListFulfillmentOptions, expected 200 / got 200, diverging key $.fulfillmentOptions[0]).

Explicitly NOT claimed as requirements anywhere in this issue: the generated fulfillmentOptionId / offerId / productId / sellerProfileId literals, and the numeric availableFromTime (the emulator derives it from its own clock). Those differ per environment and per run; the defects above are the identifier FORMAT, the list lengths and order, which optional members appear, and the fact that the content is a template rather than the stored record.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All three steps belong to fixture marketplace-discovery-search-listings-and-drill-into-listing and all three returned 200 where 200 was expected.

Cause 1 - #task#2, GetListing, 33 diverging keys. Samples: $.categories[0].categoryId expected a 36-char taxonomy UUID, got 'infrastructure-software'; $.categories[0].displayName expected 'Data Integration', got 'Infrastructure Software' (ancestors-first ordering); $.categories[2].categoryId/displayName swapped the same way; $.highlights[0] expected the seller's first highlight sentence, got 'Fulfilled as SaaS'; $.highlights[1] got 'Listed under Data Preparation'; $.highlights[2] expected a third stored highlight, got nothing; $.badges[0] extra - expected None, got {'displayName': 'Quick Launch', 'badgeType': 'QUICK_LAUNCH'}; $.associatedEntities[0] product/offer names and seller ids are template values; $.logoThumbnailUrl got 'https://example.com/logos/borealis-data-prep.png'; $.longDescription got the formatted '<short description> <name> is listed under <category> and fulfilled as <fulfillment>.' sentence.

Cause 2 - #task#3, GetProduct, 27 diverging keys: $.categories[0..3] (slug ids, ancestors-first), $.highlights[0] got 'Fulfilled as SaaS' with highlights[1] and [2] absent (got None), $.logoThumbnailUrl got 'https://example.com/logos/borealis-data-prep.png', plus $.productName, $.manufacturer.*, $.shortDescription, $.longDescription, $.resources[0..3].*, $.sellerEngagements[0..1], $.purchaseIndicators.

Cause 3 - #task#4, ListFulfillmentOptions: $.fulfillmentOptions[0] - expected saasFulfillmentOption {deploymentTemplates: [], fulfillmentOptionDisplayName 'SaaS', fulfillmentOptionType 'SAAS', fulfillmentUrl <seller URL>, quickLaunch 'DISABLED'}; got saasFulfillmentOption {fulfillmentOptionDisplayName 'SaaS', fulfillmentOptionType 'SAAS', fulfillmentUrl 'https://example.com/borealis-data-prep/register', usageInstructions 'Subscribe, then open the ... console.', launchUrl 'https://example.com/borealis-data-prep/launch', quickLaunch 'ENABLED'} and no deploymentTemplates.

Code read: src/state.rs lines 3603-3680 - the BREADTH_ROWS seeding loop that builds every one of these rows (highlights as two format! sentences, logo as 'https://example.com/logos/{slug}.png', long_description as a formatted sentence, categories via classified_under(row.categories)); src/state.rs 684-698 (listing_summary / listing_badges). Model reference: smithy_query operations GetListing, GetProduct, ListFulfillmentOptions and shapes Category, SaasFulfillmentOption, ListingAssociatedEntity on services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Run fixture marketplace-discovery-search-listings-and-drill-into-listing. Step task#1 searches and yields a listing id.
4. Step task#2 POSTs /2026-02-05/getListing with that listing id: observe slug categoryIds in ancestors-first order, exactly two generated highlights ('Fulfilled as ...', 'Listed under ...'), a QUICK_LAUNCH badge the recording does not report, and template listingName/descriptions/logoThumbnailUrl/resources/sellerEngagements/useCases.
5. Step task#3 POSTs /2026-02-05/getProduct with the product id from that listing: observe the same slug categories and generated highlights plus template product text.
6. Step task#4 POSTs /2026-02-05/listFulfillmentOptions for that product: observe the SaaS option with quickLaunch 'ENABLED', an emitted usageInstructions and launchUrl, and no deploymentTemplates.
