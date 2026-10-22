// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pricing-awspricelistservice service.
//!
//! The Price List API is a READ-ONLY catalog: no operation it models creates a
//! service, a product or a price-list file, so the catalog is SEEDED when this state
//! is first constructed (through [`Default`], which `engine.state::<T>()` calls on
//! first touch) and every handler derives its answer from it. No handler writes here.
//!
//! The one catalog is also what keeps the operations consistent with each other:
//! `DescribeServices.AttributeNames` is [`ServiceCatalog::attribute_names`],
//! `GetAttributeValues` is the distinct values those attributes take on the SAME
//! products `GetProducts` returns, and the `PriceListArn` `ListPriceLists` reports is
//! the key `GetPriceListFileUrl` resolves.
//!
//! The price-list FILES are served by the emulator itself: the first
//! `GetPriceListFileUrl` opens one loopback-bound HTTP listener through
//! `engine.listeners().http(..)` ([`ensure_file_base_url`]) and the URL it returns
//! addresses that listener, which renders the file from this same catalog
//! ([`render_price_list_file`]). A client that follows the URL therefore stays on the
//! emulator and reads emulator state; no response names a host outside it.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// The `FormatVersion` this catalog is published in.
///
/// `DescribeServices`/`GetProducts` document exactly one valid value for the member
/// (`smithy_query(query="doc",shape="FormatVersion")` — "Valid values are: aws_v1").
pub const DEFAULT_FORMAT_VERSION: &str = "aws_v1";

/// The currencies the Price List API publishes price lists in.
///
/// `ListPriceLists$CurrencyCode` is "The three alphabetical character ISO-4217
/// currency code that the Price List files are denominated in", and the API
/// publishes the AWS price lists in USD and the China (Ningxia/Beijing) price lists
/// in CNY. The set is the API's, so it does not move when the catalog is reseeded.
pub const SUPPORTED_CURRENCY_CODES: &[&str] = &["CNY", "USD"];

/// Page size used when a request names no `MaxResults`.
///
/// The model declares the member nullable with no server-side default, and the API
/// reference documents 100 results as the default page for every paginated Price List
/// operation.
const DEFAULT_MAX_RESULTS: usize = 100;

/// The `formatVersion` of a price-list file, as the file itself reports it.
const FILE_FORMAT_VERSION: &str = "v1.0";

/// The disclaimer the emulator's own price-list files carry, so a reader can tell
/// where the numbers came from.
const FILE_DISCLAIMER: &str = "This price-list file is rendered by the AWS emulator \
     from its own seeded catalog. The rates are not Amazon Web Services prices.";

/// The on-demand offer-term code AWS uses in every `aws_v1` offer document.
const ON_DEMAND_TERM_CODE: &str = "JRTCKXETXF";

/// The on-demand rate code suffix AWS uses in every `aws_v1` offer document.
const ON_DEMAND_RATE_CODE: &str = "6YS6EN2CT7";

/// The file formats each seeded price list is published in.
const SEEDED_FILE_FORMATS: &[&str] = &["json"];

/// One product offer in the catalog.
///
/// `offer_json` is the `aws_v1` product-offer document `GetProducts` returns in
/// `PriceList`. It is BUILT from `sku`/`attributes` when the catalog is seeded
/// ([`offer_document`]), so the document can never disagree with the attributes
/// `GetAttributeValues` and the `GetProducts` filters read.
#[derive(Debug, Clone)]
pub struct Product {
    /// The product's stock-keeping unit, unique within the service.
    pub sku: String,
    /// The product metadata, keyed by attribute name (e.g. `instanceType`).
    pub attributes: BTreeMap<String, String>,
    /// The full product-offer JSON document for this product.
    pub offer_json: String,
}

/// Everything the catalog holds for one service code.
#[derive(Debug, Clone)]
pub struct ServiceCatalog {
    /// The service's code, e.g. `AmazonEC2`.
    pub service_code: String,
    /// The attribute names available for this service, sorted.
    pub attribute_names: Vec<String>,
    /// The service's products, in catalog order (stable, so pagination is stable).
    pub products: Vec<Product>,
}

impl ServiceCatalog {
    /// Build a catalog whose `attribute_names` is the union of the attribute names
    /// its products carry, so every name `DescribeServices` reports has at least one
    /// value for `GetAttributeValues` to return.
    fn new(service_code: &str, products: Vec<Product>) -> Self {
        let attribute_names: BTreeSet<String> = products
            .iter()
            .flat_map(|product| product.attributes.keys().cloned())
            .collect();
        Self {
            service_code: service_code.to_string(),
            attribute_names: attribute_names.into_iter().collect(),
            products,
        }
    }

    /// Whether this service publishes `attribute_name`.
    pub fn has_attribute(&self, attribute_name: &str) -> bool {
        self.attribute_names.iter().any(|name| name == attribute_name)
    }

    /// The distinct values `attribute_name` takes across this service's products,
    /// in sorted order. Derived on read — never stored a second time.
    pub fn attribute_values(&self, attribute_name: &str) -> Vec<String> {
        self.products
            .iter()
            .filter_map(|product| product.attributes.get(attribute_name).cloned())
            .collect::<BTreeSet<String>>()
            .into_iter()
            .collect()
    }
}

/// One price-list file set: the ARN clients resolve, and the URLs it resolves to.
#[derive(Debug, Clone)]
pub struct PriceListRecord {
    /// The stored `PriceListArn`; the key `GetPriceListFileUrl` looks up.
    pub arn: String,
    /// The service the price list covers.
    pub service_code: String,
    /// The Region the price list covers, when it is Region-specific.
    pub region_code: Option<String>,
    /// The ISO-4217 currency the files are denominated in.
    pub currency_code: String,
    /// The instant from which this price list is in effect.
    pub effective_date: SystemTime,
    /// The formats the files are published in.
    pub file_formats: Vec<String>,
    /// The path, per format, this price list's file is served at on the emulator's own
    /// file listener. The URL `GetPriceListFileUrl` returns is this path under that
    /// listener's base URL, so it is the same for a given (ARN, format) on every call.
    pub file_paths: BTreeMap<String, String>,
}

impl PriceListRecord {
    /// Seconds since the Unix epoch at which this price list takes effect.
    pub fn effective_epoch_secs(&self) -> i64 {
        epoch_secs(self.effective_date)
    }

    /// This price list's effective date as an RFC 3339 timestamp.
    pub fn effective_date_text(&self) -> String {
        ::aws_smithy_types::DateTime::from_secs(self.effective_epoch_secs())
            .fmt(::aws_smithy_types::date_time::Format::DateTime)
            .unwrap_or_default()
    }

    /// The `yyyymmddHHMMSS` version stamp AWS names a price-list file set by, derived
    /// from the effective date so the two always agree.
    pub fn version(&self) -> String {
        version_stamp(&self.effective_date_text())
    }

    /// The stored file path for `file_format`, matched the way the API accepts the
    /// format (`json` for a `FileFormats` entry of `json`), or `None` when this price
    /// list is not published in that format.
    pub fn path_for_format(&self, file_format: &str) -> Option<&str> {
        self.file_paths
            .iter()
            .find(|(format, _)| format.eq_ignore_ascii_case(file_format))
            .map(|(_, path)| path.as_str())
    }
}

/// Per-service state for com-amazonaws-pricing-awspricelistservice. A handler reaches it through [`service_state`].
pub struct ComAmazonawsPricingAwspricelistserviceState {
    /// The catalog's format version, reported by `DescribeServices`/`GetProducts`.
    pub format_version: String,
    /// The catalog, keyed by service code so pagination order is stable.
    pub services: BTreeMap<String, ServiceCatalog>,
    /// The price-list records, keyed by `PriceListArn` so pagination order is stable.
    pub price_lists: BTreeMap<String, PriceListRecord>,
    /// The emulator's own price-list file listener, opened on first use. `None` until
    /// a `GetPriceListFileUrl` needs a URL.
    file_server: Option<FileServer>,
}

/// The loopback HTTP listener this service serves its price-list files from.
///
/// The state OWNS the listener: dropping it closes the port, so it must outlive every
/// URL handed out, which is the whole life of the state.
struct FileServer {
    /// `https://<loopback host>:<port>`, the base of every file URL.
    base_url: String,
    /// Held only to keep the port open; the core owns the socket and its TLS.
    _listener: ::emulator_server::ResourceListener,
}

/// Written by hand rather than derived: [`FileServer`] holds a core
/// `ResourceListener`, which is not `Debug`. The listener is reported by the address
/// it announced, which is what a reader needs.
impl std::fmt::Debug for ComAmazonawsPricingAwspricelistserviceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComAmazonawsPricingAwspricelistserviceState")
            .field("format_version", &self.format_version)
            .field("services", &self.services)
            .field("price_lists", &self.price_lists)
            .field(
                "file_server",
                &self.file_server.as_ref().map(|server| &server.base_url),
            )
            .finish()
    }
}

/// Short name for this service's state, as the handlers read it.
pub type PricingState = ComAmazonawsPricingAwspricelistserviceState;

impl Default for ComAmazonawsPricingAwspricelistserviceState {
    fn default() -> Self {
        seeded_catalog()
    }
}

impl ComAmazonawsPricingAwspricelistserviceState {
    /// The catalog for `service_code`, or `None` when the catalog holds no such
    /// service.
    pub fn catalog(&self, service_code: &str) -> Option<&ServiceCatalog> {
        self.services.get(service_code)
    }

    /// Whether the Price List API publishes price lists in `currency_code`.
    ///
    /// The accepted set is a property of the API, not of this catalog's contents:
    /// `ListPriceLists$CurrencyCode` documents "The three alphabetical character
    /// ISO-4217 currency code that the Price List files are denominated in", and the
    /// Price List API publishes exactly USD and CNY ([`SUPPORTED_CURRENCY_CODES`]).
    /// A supported currency that no stored record happens to use is therefore an
    /// empty page, not a bad parameter.
    pub fn publishes_currency(currency_code: &str) -> bool {
        SUPPORTED_CURRENCY_CODES.contains(&currency_code)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>> {
    shared_engine(engine)
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPricingAwspricelistserviceState>)
}

/// The one shared engine behind a handler's `Eng`, which is also what vends the
/// listener a price-list file is served from.
fn shared_engine<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}

// ---------------------------------------------------------------------------
// The price-list file listener
// ---------------------------------------------------------------------------

/// The base URL of this service's price-list file listener, opening it on first use.
///
/// The listener is the emulator's own, loopback-bound and TLS-terminated by the core
/// (`engine.listeners().http(..)`), and it renders each file from this catalog — so a
/// client that follows a `GetPriceListFileUrl` answer reads emulator state and never
/// leaves the machine. Opening it changes no catalog state and is idempotent: the
/// first listener is kept, so a given (ARN, format) resolves to the same URL for the
/// life of the emulator.
pub async fn ensure_file_base_url<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>,
) -> Option<String> {
    let existing = state
        .lock()
        .file_server
        .as_ref()
        .map(|server| server.base_url.clone());
    if let Some(base_url) = existing {
        return Some(base_url);
    }

    let listener = shared_engine(engine)?
        .listeners()
        .http(
            PriceListFileServer {
                state: state.clone(),
            },
            ::emulator_server::HttpPolicy::default(),
        )
        .await
        .ok()?;
    let base_url = format!("https://{}", authority(&listener));

    let mut guard = state.lock();
    // Another request may have opened one while this one was binding; keep the first,
    // so the URL for an (ARN, format) never changes under a caller. Dropping the
    // loser closes its port.
    if let Some(server) = &guard.file_server {
        return Some(server.base_url.clone());
    }
    guard.file_server = Some(FileServer {
        base_url: base_url.clone(),
        _listener: listener,
    });
    Some(base_url)
}

/// The `host:port` a URL addresses the listener by, with an IPv6 host bracketed as
/// RFC 3986 requires (`[::1]:8443`), so the URL parses wherever the operator bound it.
fn authority(listener: &::emulator_server::ResourceListener) -> String {
    let host = listener.host();
    let port = listener.port();
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Serves the price-list files on the emulator's own listener. The core runs the
/// browser checks and owns CORS; this only renders the file a path names.
struct PriceListFileServer {
    state: ::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>,
}

impl ::emulator_server::HttpHandler for PriceListFileServer {
    fn handle(
        &self,
        req: ::http::Request<Vec<u8>>,
    ) -> Pin<Box<dyn Future<Output = ::http::Response<Vec<u8>>> + Send>> {
        let state = self.state.clone();
        let method = req.method().clone();
        let path = req.uri().path().to_string();
        Box::pin(async move {
            if method != ::http::Method::GET && method != ::http::Method::HEAD {
                return file_response(405, "text/plain", b"only GET and HEAD are served".to_vec());
            }
            match render_price_list_file(&state, &path) {
                Some((content_type, body)) => {
                    let body = if method == ::http::Method::HEAD {
                        Vec::new()
                    } else {
                        body.into_bytes()
                    };
                    file_response(200, content_type, body)
                }
                None => file_response(404, "text/plain", b"no price-list file at this path".to_vec()),
            }
        })
    }
}

/// Build one file response.
fn file_response(status: u16, content_type: &str, body: Vec<u8>) -> ::http::Response<Vec<u8>> {
    ::http::Response::builder()
        .status(status)
        .header("content-type", content_type)
        .body(body)
        // Every argument is this function's own constant or a built body, so the
        // builder cannot fail; answer a bare 500 rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Vec::new());
            *response.status_mut() = ::http::StatusCode::INTERNAL_SERVER_ERROR;
            response
        })
}

/// The price-list file served at `path`, as `(content type, body)`, or `None` when no
/// price list in the catalog publishes a file there.
///
/// The body is rendered from the catalog on each read — the products the price list
/// covers and the offer documents `GetProducts` returns for them — so the file and the
/// API can never disagree.
pub fn render_price_list_file(
    state: &::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>,
    path: &str,
) -> Option<(&'static str, String)> {
    let state = state.lock();
    let (record, file_format) = state.price_lists.values().find_map(|record| {
        record
            .file_paths
            .iter()
            .find(|(_, file_path)| file_path.as_str() == path)
            .map(|(format, _)| (record, format.clone()))
    })?;
    let catalog = state.catalog(&record.service_code)?;
    let products: Vec<&Product> = catalog
        .products
        .iter()
        .filter(|product| covers(record, product))
        .collect();

    match file_format.as_str() {
        "json" => Some(("application/json", price_list_file_json(record, &products))),
        // Only the formats a record publishes have a path, so this is unreachable
        // unless a format is added to a record without a renderer for it.
        _ => None,
    }
}

/// Whether `product` belongs in `record`'s file: a Region-specific price list covers
/// the products of that Region, and one with no Region covers the service's products.
fn covers(record: &PriceListRecord, product: &Product) -> bool {
    match (&record.region_code, product.attributes.get("regionCode")) {
        (Some(region), Some(product_region)) => region == product_region,
        (Some(_), None) => false,
        (None, _) => true,
    }
}

/// The `v1.0` price-list file: the file header, every product it covers, and the terms
/// that price them, taken from the products' own offer documents.
fn price_list_file_json(record: &PriceListRecord, products: &[&Product]) -> String {
    let mut product_blocks = serde_json::Map::new();
    let mut on_demand = serde_json::Map::new();
    for product in products {
        let Ok(offer) = serde_json::from_str::<serde_json::Value>(&product.offer_json) else {
            continue;
        };
        if let Some(block) = offer.get("product") {
            product_blocks.insert(product.sku.clone(), block.clone());
        }
        if let Some(terms) = offer.pointer("/terms/OnDemand") {
            on_demand.insert(product.sku.clone(), terms.clone());
        }
    }
    serde_json::json!({
        "formatVersion": FILE_FORMAT_VERSION,
        "disclaimer": FILE_DISCLAIMER,
        "offerCode": record.service_code,
        "version": record.version(),
        "publicationDate": record.effective_date_text(),
        "currencyCode": record.currency_code,
        "regionCode": record.region_code,
        "products": product_blocks,
        "terms": { "OnDemand": on_demand },
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// A `NextToken` that did not decode, or that belongs to a different query.
///
/// Both are the same answer to a caller: the modelled `InvalidNextTokenException`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidToken;

/// A short, stable fingerprint of the query a page belongs to.
///
/// A `NextToken` carries it so a token minted for one query (`AmazonEC2` filtered on
/// `instanceType`) cannot be replayed against another, where the same offset would
/// name an unrelated item.
pub fn query_fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.as_bytes());
        // A separator, so ["a", "bc"] and ["ab", "c"] do not fingerprint alike.
        hasher.update([0u8]);
    }
    hex::encode(hasher.finalize())[..16].to_string()
}

/// The opaque token for "continue this query at `offset`".
pub fn encode_next_token(fingerprint: &str, offset: usize) -> String {
    hex::encode(format!("{offset}:{fingerprint}"))
}

/// The offset `token` encodes, when it is a token this service minted for the query
/// `fingerprint` identifies.
pub fn decode_next_token(token: &str, fingerprint: &str) -> Result<usize, InvalidToken> {
    let raw = hex::decode(token).map_err(|_| InvalidToken)?;
    let text = String::from_utf8(raw).map_err(|_| InvalidToken)?;
    let (offset, token_fingerprint) = text.split_once(':').ok_or(InvalidToken)?;
    if token_fingerprint != fingerprint {
        return Err(InvalidToken);
    }
    offset.parse::<usize>().map_err(|_| InvalidToken)
}

/// One page of `items`: the slice to return, and the token for the page after it
/// (`None` on the last page).
///
/// `items` must be the result set of the query `fingerprint` identifies, in a stable
/// order, so the same token always names the same boundary.
pub fn paginate<'a, T>(
    items: &'a [T],
    fingerprint: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<(&'a [T], Option<String>), InvalidToken> {
    let offset = match next_token {
        Some(token) => decode_next_token(token, fingerprint)?,
        None => 0,
    };
    if offset > items.len() {
        // The token decoded, but names a boundary past the end of this result set.
        return Err(InvalidToken);
    }
    let limit = max_results
        .and_then(|max| usize::try_from(max).ok())
        .filter(|max| *max > 0)
        .unwrap_or(DEFAULT_MAX_RESULTS);
    let end = items.len().min(offset.saturating_add(limit));
    let token = (end < items.len()).then(|| encode_next_token(fingerprint, end));
    Ok((&items[offset..end], token))
}

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// One seeded product, before its offer document is built.
struct ProductSeed {
    sku: &'static str,
    product_family: &'static str,
    unit: &'static str,
    price_usd: &'static str,
    attributes: &'static [(&'static str, &'static str)],
}

/// One seeded price list, before its ARN and file paths are built.
struct PriceListSeed {
    service_code: &'static str,
    region_code: &'static str,
    currency_code: &'static str,
    /// The instant the price list takes effect, as an RFC 3339 timestamp. The
    /// price-list file `version` stamp is derived from it, so the two agree.
    effective_date: &'static str,
}

/// The catalog every emulator starts with.
fn seeded_catalog() -> ComAmazonawsPricingAwspricelistserviceState {
    let mut services = BTreeMap::new();
    for (service_code, seeds) in [
        ("AmazonEC2", EC2_PRODUCTS),
        ("AmazonS3", S3_PRODUCTS),
    ] {
        let products = seeds
            .iter()
            .map(|seed| product_from_seed(service_code, seed))
            .collect();
        services.insert(
            service_code.to_string(),
            ServiceCatalog::new(service_code, products),
        );
    }

    let mut price_lists = BTreeMap::new();
    for seed in SEEDED_PRICE_LISTS {
        let record = price_list_from_seed(seed);
        price_lists.insert(record.arn.clone(), record);
    }

    ComAmazonawsPricingAwspricelistserviceState {
        format_version: DEFAULT_FORMAT_VERSION.to_string(),
        services,
        price_lists,
        // Opened on the first GetPriceListFileUrl, not at construction: opening a
        // listener is asynchronous and needs the shared engine.
        file_server: None,
    }
}

/// Build a catalog product, including the offer document returned for it.
fn product_from_seed(service_code: &str, seed: &ProductSeed) -> Product {
    let attributes: BTreeMap<String, String> = seed
        .attributes
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect();
    let offer_json = offer_document(service_code, seed, &attributes);
    Product {
        sku: seed.sku.to_string(),
        attributes,
        offer_json,
    }
}

/// The `aws_v1` product-offer document for one product: the `product` block with its
/// attributes, and the on-demand term that prices it.
fn offer_document(
    service_code: &str,
    seed: &ProductSeed,
    attributes: &BTreeMap<String, String>,
) -> String {
    let sku = seed.sku;
    let term_key = format!("{sku}.{ON_DEMAND_TERM_CODE}");
    let rate_code = format!("{term_key}.{ON_DEMAND_RATE_CODE}");
    let effective = CATALOG_PUBLICATION_DATE;

    // The term and rate keys are the SKU-derived codes AWS keys these maps by, so
    // they are inserted rather than written as literals.
    let mut price_dimensions = serde_json::Map::new();
    price_dimensions.insert(
        rate_code.clone(),
        serde_json::json!({
            "rateCode": rate_code,
            "unit": seed.unit,
            "beginRange": "0",
            "endRange": "Inf",
            "appliesTo": [],
            "description": format!("${} per {} of {sku}", seed.price_usd, seed.unit),
            "pricePerUnit": { "USD": seed.price_usd },
        }),
    );
    let mut on_demand = serde_json::Map::new();
    on_demand.insert(
        term_key,
        serde_json::json!({
            "sku": sku,
            "offerTermCode": ON_DEMAND_TERM_CODE,
            "effectiveDate": effective,
            "termAttributes": {},
            "priceDimensions": price_dimensions,
        }),
    );

    serde_json::json!({
        "product": {
            "productFamily": seed.product_family,
            "attributes": attributes,
            "sku": sku,
        },
        "serviceCode": service_code,
        "terms": { "OnDemand": on_demand },
        "version": version_stamp(effective),
        "publicationDate": effective,
    })
    .to_string()
}

/// Build a price-list record: its ARN, the formats it is published in, and the URL
/// for each of those formats. Every one is derived from the seed, so the ARN
/// `ListPriceLists` reports resolves in `GetPriceListFileUrl` and the URL is the same
/// on every call.
fn price_list_from_seed(seed: &PriceListSeed) -> PriceListRecord {
    let partition = partition_for(seed.currency_code);
    let version = version_stamp(seed.effective_date);
    let arn = format!(
        // Not `engine::emulator_arn`: a Price List ARN carries neither a Region nor an
        // account id (the modelled pattern is `^arn:…:pricing:::price-list/…$`), which
        // that helper always writes.
        "arn:aws:pricing:::price-list/{partition}/{}/{}/{version}/{}",
        seed.service_code, seed.currency_code, seed.region_code
    );
    let file_formats: Vec<String> = SEEDED_FILE_FORMATS
        .iter()
        .map(|format| (*format).to_string())
        .collect();
    // The path the file is served at on the emulator's own listener, laid out the way
    // the Price List offer files are: one per format, per service, version and Region.
    let file_paths = file_formats
        .iter()
        .map(|format| {
            let path = format!(
                "/offers/v1.0/{partition}/{}/{version}/{}/index.{format}",
                seed.service_code, seed.region_code
            );
            (format.clone(), path)
        })
        .collect();
    PriceListRecord {
        arn,
        service_code: seed.service_code.to_string(),
        region_code: Some(seed.region_code.to_string()),
        currency_code: seed.currency_code.to_string(),
        effective_date: instant_of(seed.effective_date),
        file_formats,
        file_paths,
    }
}

/// The partition a price list belongs to, from the currency it is denominated in:
/// the Price List API publishes `CNY` files for the China partition and `USD` files
/// for every other Region.
fn partition_for(currency_code: &str) -> &'static str {
    if currency_code == "CNY" {
        "cn"
    } else {
        "aws"
    }
}

/// The `yyyymmddHHMMSS` version stamp AWS names a price-list file set by, derived
/// from its RFC 3339 effective date so the two always agree.
fn version_stamp(rfc3339: &str) -> String {
    rfc3339.chars().filter(char::is_ascii_digit).collect()
}

/// Parse a seed's RFC 3339 timestamp into an instant.
fn instant_of(rfc3339: &str) -> SystemTime {
    let secs = ::aws_smithy_types::DateTime::from_str(
        rfc3339,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .map_or(0, |parsed| parsed.secs());
    UNIX_EPOCH + Duration::from_secs(u64::try_from(secs).unwrap_or(0))
}

/// Seconds since the Unix epoch, for comparison against a request's timestamp.
fn epoch_secs(instant: SystemTime) -> i64 {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(since) => i64::try_from(since.as_secs()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_secs())
            .map_or(i64::MIN, |secs| -secs),
    }
}

/// The publication date of the seeded catalog's offer documents.
const CATALOG_PUBLICATION_DATE: &str = "2024-01-01T00:00:00Z";

/// The seeded `AmazonEC2` products. Attribute names and values are the ones the
/// real EC2 offer file carries for an on-demand compute instance.
const EC2_PRODUCTS: &[ProductSeed] = &[
    ProductSeed {
        sku: "T3MICROUSE1LINUX",
        product_family: "Compute Instance",
        unit: "Hrs",
        price_usd: "0.0104000000",
        attributes: &[
            ("capacitystatus", "Used"),
            ("instanceFamily", "General purpose"),
            ("instanceType", "t3.micro"),
            ("location", "US East (N. Virginia)"),
            ("locationType", "AWS Region"),
            ("memory", "1 GiB"),
            ("operatingSystem", "Linux"),
            ("operation", "RunInstances"),
            ("preInstalledSw", "NA"),
            ("regionCode", "us-east-1"),
            ("servicecode", "AmazonEC2"),
            ("tenancy", "Shared"),
            ("termType", "OnDemand"),
            ("usagetype", "BoxUsage:t3.micro"),
            ("vcpu", "2"),
        ],
    },
    ProductSeed {
        sku: "T3SMALLUSE1LINUX",
        product_family: "Compute Instance",
        unit: "Hrs",
        price_usd: "0.0208000000",
        attributes: &[
            ("capacitystatus", "Used"),
            ("instanceFamily", "General purpose"),
            ("instanceType", "t3.small"),
            ("location", "US East (N. Virginia)"),
            ("locationType", "AWS Region"),
            ("memory", "2 GiB"),
            ("operatingSystem", "Linux"),
            ("operation", "RunInstances"),
            ("preInstalledSw", "NA"),
            ("regionCode", "us-east-1"),
            ("servicecode", "AmazonEC2"),
            ("tenancy", "Shared"),
            ("termType", "OnDemand"),
            ("usagetype", "BoxUsage:t3.small"),
            ("vcpu", "2"),
        ],
    },
    ProductSeed {
        sku: "M5LARGEUSE1LINUX",
        product_family: "Compute Instance",
        unit: "Hrs",
        price_usd: "0.0960000000",
        attributes: &[
            ("capacitystatus", "Used"),
            ("instanceFamily", "General purpose"),
            ("instanceType", "m5.large"),
            ("location", "US East (N. Virginia)"),
            ("locationType", "AWS Region"),
            ("memory", "8 GiB"),
            ("operatingSystem", "Linux"),
            ("operation", "RunInstances"),
            ("preInstalledSw", "NA"),
            ("regionCode", "us-east-1"),
            ("servicecode", "AmazonEC2"),
            ("tenancy", "Shared"),
            ("termType", "OnDemand"),
            ("usagetype", "BoxUsage:m5.large"),
            ("vcpu", "2"),
        ],
    },
    ProductSeed {
        sku: "T3MICROEUW1LINUX",
        product_family: "Compute Instance",
        unit: "Hrs",
        price_usd: "0.0114000000",
        attributes: &[
            ("capacitystatus", "Used"),
            ("instanceFamily", "General purpose"),
            ("instanceType", "t3.micro"),
            ("location", "EU (Ireland)"),
            ("locationType", "AWS Region"),
            ("memory", "1 GiB"),
            ("operatingSystem", "Linux"),
            ("operation", "RunInstances"),
            ("preInstalledSw", "NA"),
            ("regionCode", "eu-west-1"),
            ("servicecode", "AmazonEC2"),
            ("tenancy", "Shared"),
            ("termType", "OnDemand"),
            ("usagetype", "EU-BoxUsage:t3.micro"),
            ("vcpu", "2"),
        ],
    },
    ProductSeed {
        sku: "M5LARGEUSW2WINDOWS",
        product_family: "Compute Instance",
        unit: "Hrs",
        price_usd: "0.1920000000",
        attributes: &[
            ("capacitystatus", "Used"),
            ("instanceFamily", "General purpose"),
            ("instanceType", "m5.large"),
            ("location", "US West (Oregon)"),
            ("locationType", "AWS Region"),
            ("memory", "8 GiB"),
            ("operatingSystem", "Windows"),
            ("operation", "RunInstances:0002"),
            ("preInstalledSw", "NA"),
            ("regionCode", "us-west-2"),
            ("servicecode", "AmazonEC2"),
            ("tenancy", "Shared"),
            ("termType", "OnDemand"),
            ("usagetype", "USW2-BoxUsage:m5.large"),
            ("vcpu", "2"),
        ],
    },
];

/// The seeded `AmazonS3` products, with the attribute names the real S3 offer file
/// carries for storage.
const S3_PRODUCTS: &[ProductSeed] = &[
    ProductSeed {
        sku: "S3STDUSE1BYTEHRS",
        product_family: "Storage",
        unit: "GB-Mo",
        price_usd: "0.0230000000",
        attributes: &[
            ("availability", "99.99%"),
            ("durability", "99.999999999%"),
            ("location", "US East (N. Virginia)"),
            ("locationType", "AWS Region"),
            ("regionCode", "us-east-1"),
            ("servicecode", "AmazonS3"),
            ("storageClass", "General Purpose"),
            ("termType", "OnDemand"),
            ("usagetype", "TimedStorage-ByteHrs"),
            ("volumeType", "Standard"),
        ],
    },
    ProductSeed {
        sku: "S3IAUSE1BYTEHRS",
        product_family: "Storage",
        unit: "GB-Mo",
        price_usd: "0.0125000000",
        attributes: &[
            ("availability", "99.9%"),
            ("durability", "99.999999999%"),
            ("location", "US East (N. Virginia)"),
            ("locationType", "AWS Region"),
            ("regionCode", "us-east-1"),
            ("servicecode", "AmazonS3"),
            ("storageClass", "Infrequent Access"),
            ("termType", "OnDemand"),
            ("usagetype", "TimedStorage-SIA-ByteHrs"),
            ("volumeType", "Standard - Infrequent Access"),
        ],
    },
    ProductSeed {
        sku: "S3STDEUW1BYTEHRS",
        product_family: "Storage",
        unit: "GB-Mo",
        price_usd: "0.0245000000",
        attributes: &[
            ("availability", "99.99%"),
            ("durability", "99.999999999%"),
            ("location", "EU (Ireland)"),
            ("locationType", "AWS Region"),
            ("regionCode", "eu-west-1"),
            ("servicecode", "AmazonS3"),
            ("storageClass", "General Purpose"),
            ("termType", "OnDemand"),
            ("usagetype", "EU-TimedStorage-ByteHrs"),
            ("volumeType", "Standard"),
        ],
    },
];

/// The seeded price-list file sets. `USD` price lists cover the commercial
/// partition's Regions and `CNY` ones the China partition's, as the Price List API
/// publishes them.
const SEEDED_PRICE_LISTS: &[PriceListSeed] = &[
    PriceListSeed {
        service_code: "AmazonEC2",
        region_code: "us-east-1",
        currency_code: "USD",
        effective_date: "2023-01-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonEC2",
        region_code: "eu-west-1",
        currency_code: "USD",
        effective_date: "2023-01-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonEC2",
        region_code: "us-west-2",
        currency_code: "USD",
        effective_date: "2023-01-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonEC2",
        region_code: "us-east-1",
        currency_code: "USD",
        effective_date: "2024-06-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonEC2",
        region_code: "cn-north-1",
        currency_code: "CNY",
        effective_date: "2023-01-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonS3",
        region_code: "us-east-1",
        currency_code: "USD",
        effective_date: "2023-01-01T00:00:00Z",
    },
    PriceListSeed {
        service_code: "AmazonS3",
        region_code: "eu-west-1",
        currency_code: "USD",
        effective_date: "2023-01-01T00:00:00Z",
    },
];
