Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-discovery-search-facets-refine-search
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-discovery-search-facets-refine-search marketplace-discovery-search-listings-and-drill-into-listing marketplace-discovery-search-listings-pagination
Issue-Shape-Id: com.amazonaws.marketplacediscovery#AWSMarketplaceDiscovery
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].logoThumbnailUrl
Issue-Key: marketplace-discovery|SearchListings|2xx->2xx|$.listingSummaries[*].reviewSummary.reviewSourceSummaries[*].sourceUrl
Issue-Related: 20261109T035256Z-marketplace-discovery-searchlistings-nexttoken-is-a-base64-of-a-plaintext.md
Issue-Commit: 5386ff6290ae18fab3ce660917360d5fb6557b5e

# SearchListings listing summaries announce an unservable placeholder logo host and a constant review sourceUrl

> _Note: this issue was identified against the code base at commit `5386ff6290ae18fab3ce660917360d5fb6557b5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchListings

## Description
This issue bundles 3 causes, all in marketplace-discovery, all three the SAME two defects in the ListingSummary projection SearchListings returns, observed in three different fixtures (a facet-refined search, the search that opens the drill-down flow, and a paginated search). Each cause is one replay cluster and each needs its own re-replay to be confirmed closed. In every one the emulator answers 200 with the right number of summaries, but two URL members of each summary are machine-generated rather than read off the listing record: logoThumbnailUrl is minted on a third-party placeholder host the emulator does not serve, and reviewSummary.reviewSourceSummaries[].sourceUrl is one hard-coded landing-page URL emitted for every listing, including listings whose record carries no sourceUrl at all.

Cause 1 of 3: a facet-refined SearchListings page returns ten summaries whose logoThumbnailUrl is a fabricated placeholder-host URL and whose review sourceUrl is a constant.
AWS: Smithy SearchListings (POST /2026-02-05/searchListings, output listingSummaries ListingSummaryList @required) returns ListingSummary{logoThumbnailUrl URL @required @length(1..2048) @pattern ^https?://.*$ - documented "The URL of the logo thumbnail image for the listing", reviewSummary ReviewSummary @required of ReviewSourceSummary{sourceName @required, sourceId @required, averageRating @required, totalReviews @required, sourceUrl URL OPTIONAL - documented "The URL where the reviews can be accessed at the source"}}. The model constrains the shape, not the literal host, so for content this is recorded behavior only: logoThumbnailUrl is the URL at which the service actually serves that listing's logo image (in the recording, a per-listing path on the marketplace's own media CDN, one path per listing, derived from the listing's stored media record and not from its name), and sourceUrl, because it is OPTIONAL, is PRESENT only when the review source has a reviews page for that listing - and when present it is the deep link to THAT listing's reviews (the recording shows .../marketplace/reviews/reviews-list/<listing's own review-source id>), never a shared landing page. Listings whose record has no review page omit the member entirely.
Emulator: 200, but every summary's logoThumbnailUrl is format!("https://example.com/logos/{slug}.png") - a URL on example.com, a reserved placeholder domain the emulator neither owns nor is reached on, so it announces an address other than its own data listener and nothing serves the image; and every summary's reviewSummary.reviewSourceSummaries[0].sourceUrl is the single constant "https://aws.amazon.com/marketplace/reviews", emitted identically for all ten listings and emitted even for the listings whose recorded summary has NO sourceUrl member (expected None, got that constant).
Why: a client that renders search results fetches logoThumbnailUrl; against AWS it gets an image, against the emulator it gets a request to a host the emulator does not serve, so every result tile fails to load and the failure looks like a client or network fault rather than emulator fidelity. sourceUrl is the control a client turns into a "read the reviews" link and its ABSENCE is what tells the client not to render that link at all: a constant value in every summary makes the link always appear, and makes every listing link to the same page, so a client cannot distinguish a listing with reviews-at-source from one without.
Fix: the SearchListings summary projection and the catalog seeding behind it in the marketplace-discovery crate's state (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/state.rs): the per-row logo URL must be a URL the emulator itself serves for that listing rather than a slug-built example.com path, and the review-source summary must carry sourceUrl only when the stored record has one, per listing, rather than a shared constant. listing_summary() restates both members straight off the record, so the defect is in what is seeded/derived, not in the handler's copy.
Traces: marketplace-discovery-search-facets-refine-search#task#3 (SearchListings, expected 200 / got 200; 20 diverging keys - logoThumbnailUrl and reviewSummary.reviewSourceSummaries[0].sourceUrl on each of listingSummaries[0..9]).

Cause 2 of 3: the SearchListings step that opens the drill-down flow has the same two defects, and there sourceUrl is expected ABSENT on every one of the five summaries.
AWS: same model members and same recorded behavior as cause 1. In this recording NONE of the five listings carries a review sourceUrl, so the member is absent from all five reviewSourceSummaries entries, and each logoThumbnailUrl is that listing's own media-CDN path.
Emulator: 200 with the five summaries, each carrying "https://example.com/logos/<slug>.png" as logoThumbnailUrl and the constant "https://aws.amazon.com/marketplace/reviews" as sourceUrl where the record has none (expected None, got the constant, on all five).
Why: this is the entry point of the search -> listing -> product -> fulfillment flow, so the first page a client sees is already announcing an unservable image host, and it over-reports a reviews link for five listings that have none.
Fix: same component as cause 1.
Traces: marketplace-discovery-search-listings-and-drill-into-listing#task#1 (SearchListings, expected 200 / got 200; 10 diverging keys - logoThumbnailUrl and reviewSummary.reviewSourceSummaries[0].sourceUrl on listingSummaries[0..4]).

Cause 3 of 3: the paginated SearchListings page has the same two defects on its first summary.
AWS: same model members and same recorded behavior as cause 1; here the first listing DOES have a review page, so its sourceUrl is the deep link to that listing's own reviews list, and its logoThumbnailUrl is its own media-CDN path.
Emulator: 200, but listingSummaries[0].logoThumbnailUrl is "https://example.com/logos/<slug>.png" and listingSummaries[0].reviewSummary.reviewSourceSummaries[0].sourceUrl is the constant "https://aws.amazon.com/marketplace/reviews" instead of a per-listing reviews link.
Why: same as cause 1; additionally, a paginating client walking many pages sees the same review URL on every listing of every page, so the member carries no information at all.
Fix: same component as cause 1.
Traces: marketplace-discovery-search-listings-pagination#task#1 (SearchListings, expected 200 / got 200), and its clustered sibling #task#2. NOTE: this cluster's diverging-key set also contains $.nextToken; that key is the subject of open issue 20261109T035256Z-marketplace-discovery-searchlistings-nexttoken-is-a-base64-of-a-plaintext.md and is NOT claimed here - this cause claims only the two listingSummaries URL members. The recorded nextToken is '<shaped-not-asserted>' (the fixture asserts its shape, not its value), so its literal value is not a requirement.

Explicitly NOT claimed as requirements anywhere in this issue: the recorded CloudFront host and path literals, the recorded review-list ids, the listing ids and slugs, and the nextToken value. Those are per-environment and per-run. The defects are (a) that logoThumbnailUrl points at a host the emulator does not serve instead of one it does, and (b) that sourceUrl is a single shared constant and is present on listings whose record omits the optional member.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator up, score 53.6%, 10 of 28 steps mismatched). All three clusters are SearchListings, all returned 200 where 200 was expected, so the divergence is entirely in the body. Divergence verdict for every key below is host_not_own.

Cause 1 - marketplace-discovery-search-facets-refine-search#task#3, 20 diverging keys. Samples, expected vs actual:
- $.listingSummaries[0].logoThumbnailUrl: expected 'https://d7umqicpi7263.cloudfront.net/img/product/2025467d-e9b1-4af9-af5d-a29d751e5138.com/950c4c976d9920f963bde1320f8e7bdc', got 'https://example.com/logos/healthcare-suite.png'.
- $.listingSummaries[0].reviewSummary.reviewSourceSummaries[0].sourceUrl: expected 'https://aws.amazon.com/marketplace/reviews/reviews-list/prodview-oo2o5363yebmo' (a per-listing deep link), got 'https://aws.amazon.com/marketplace/reviews' (the shared landing page).
- $.listingSummaries[1].reviewSummary.reviewSourceSummaries[0].sourceUrl: expected None (member absent), got 'https://aws.amazon.com/marketplace/reviews'.
- $.listingSummaries[2].reviewSummary.reviewSourceSummaries[0].sourceUrl: expected 'https://aws.amazon.com/marketplace/reviews/reviews-list/B08YLQM9NY', got the same constant.
- $.listingSummaries[1..9].logoThumbnailUrl: all 'https://example.com/logos/<slug>.png' ('financial-services-platform', 'training-studio', 'managed-services-gateway', 'implementation-toolkit', ...) where the record has a per-listing media path.

Cause 2 - marketplace-discovery-search-listings-and-drill-into-listing#task#1, 10 diverging keys: $.listingSummaries[0..4].logoThumbnailUrl all 'https://example.com/logos/<slug>.png' ('structured-data-ml-service', 'telecommunications-data-gateway', 'public-sector-data-toolkit', 'environmental-data-workbench', 'healthcare-life-sciences-data-hub'), and $.listingSummaries[0..4].reviewSummary.reviewSourceSummaries[0].sourceUrl expected None on all five, got 'https://aws.amazon.com/marketplace/reviews' on all five.

Cause 3 - marketplace-discovery-search-listings-pagination#task#1 (clustered with #task#2): $.listingSummaries[0].logoThumbnailUrl expected the listing's media-CDN path, got 'https://example.com/logos/nimbus-firewall.png'; $.listingSummaries[0].reviewSummary.reviewSourceSummaries[0].sourceUrl expected 'https://aws.amazon.com/marketplace/reviews/reviews-list/prodview-btyciyjmdewhm', got 'https://aws.amazon.com/marketplace/reviews'. ($.nextToken also diverges in this cluster and is owned by the open nextToken issue, not by this one.)

Code read (services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/src/state.rs): listing_summary() at 688-713 restates logo_thumbnail_url (698) and review_summary (701) directly from the Listing record, so the generated values are seeded, not projected: seed_rows builds the logo as `let logo = format!("https://example.com/logos/{slug}.png")` at 3725 and stores it at 3741 (and reuses the same value for the product at 3779), and sets `review_summary: reviews(rating, total_reviews)` at 3768 from the (averageRating, totalReviews) pair in BREADTH_REVIEWS (3508-3516) - the reviews() helper supplies the source URL, which is why it is identical for every row and present even where the record should omit it. Model reference: smithy_query query=operation operation=SearchListings and query=shape shape=ListingSummary,ReviewSummary,ReviewSourceSummary,URL on services/service-com-amazonaws-marketplacediscovery-awsmarketplacediscovery/smithy-model/com-amazonaws-marketplacediscovery-awsmarketplacediscovery.json (sourceUrl is NOT marked required there).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: run fixture marketplace-discovery-search-facets-refine-search, step task#3 (POST /2026-02-05/searchListings with the refined facet filters). Observe 200 with ten summaries whose logoThumbnailUrl is all 'https://example.com/logos/<slug>.png' and whose reviewSummary.reviewSourceSummaries[0].sourceUrl is the identical constant 'https://aws.amazon.com/marketplace/reviews' - including summaries [1] and [3], whose recorded response has no sourceUrl member.
4. Try to GET one of the returned logoThumbnailUrl values: it addresses example.com, not the emulator's data listener, so no image is served.
5. Cause 2: run fixture marketplace-discovery-search-listings-and-drill-into-listing, step task#1. Observe the same two substitutions on all five summaries, with sourceUrl expected absent on every one.
6. Cause 3: run fixture marketplace-discovery-search-listings-pagination, step task#1 (and its clustered sibling task#2). Observe the same two substitutions on listingSummaries[0].
7. Confirm the fix by re-replaying all three bound steps: marketplace-discovery-search-facets-refine-search#task#3, marketplace-discovery-search-listings-and-drill-into-listing#task#1 and marketplace-discovery-search-listings-pagination#task#1.
