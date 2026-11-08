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

/// The message every paginated Price List operation refuses an unusable
/// `NextToken` with.
///
/// The sentence carries no request data and does not vary with the token, so it is
/// an API constant for this condition rather than a per-request message — trailing
/// space included, as the service sends it.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "NextToken is invalid. ";

/// The message `ListPriceLists` refuses a pattern-valid currency the API publishes
/// no price lists in with.
///
/// AWS answers that condition as an authorization outcome rather than a bad
/// parameter, with a fixed sentence carrying no request data.
pub const CURRENCY_NOT_AUTHORIZED_MESSAGE: &str =
    "Caller is not authorized to access requested price lists";

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

/// The rate-code suffix AWS uses for a term's per-unit price dimension in every
/// `aws_v1` offer document. A term is identified by its offer term code, so the
/// on-demand and the reserved dimensions share this suffix.
const PRICE_DIMENSION_RATE_CODE: &str = "6YS6EN2CT7";

/// The rate-code suffix AWS uses for the one-off upfront fee of a reserved term bought
/// with an upfront payment. It is a SECOND dimension beside the term's usage dimension,
/// so it carries its own suffix.
const UPFRONT_FEE_RATE_CODE: &str = "2TG2D8R56U";

/// The description AWS gives that upfront-fee dimension. It states no rate: the fee is
/// the dimension's own `pricePerUnit`.
const UPFRONT_FEE_DESCRIPTION: &str = "Upfront Fee";

/// The unit AWS states an upfront fee in — a one-off quantity rather than a usage rate.
const UPFRONT_FEE_UNIT: &str = "Quantity";

/// The hours a reservation year is billed over, for turning a lease's hourly cost into
/// the upfront fee a purchase option pays for it (365 days of 24 hours).
const HOURS_PER_LEASE_YEAR: u64 = 8_760;

/// The file formats each seeded price list is published in.
///
/// `GetPriceListFileUrl$FileFormat` is documented as a format "obtained from the
/// ListPriceLists response", so this list is a CONTRACT: every format named here is one
/// this catalog renders a file for ([`render_price_list_file`]) and
/// `GetPriceListFileUrl` resolves. The Price List API publishes each price list both as
/// the `aws_v1` offer document and as the flat CSV form of the same data; the offer
/// document is the canonical form, so it is listed first.
const SEEDED_FILE_FORMATS: &[&str] = &["json", "csv"];

/// The fixed leading columns of a price-list file's CSV form: the identity of one price
/// dimension, the term it belongs to and the rate it charges. The covered products' own
/// attribute names follow, one column each, so the flat form carries every dimension
/// the nested form does.
const CSV_DIMENSION_COLUMNS: &[&str] = &[
    "SKU",
    "OfferTermCode",
    "RateCode",
    "TermType",
    "PriceDescription",
    "EffectiveDate",
    "StartingRange",
    "EndingRange",
    "Unit",
    "PricePerUnit",
    "Currency",
    "LeaseContractLength",
    "PurchaseOption",
    "OfferingClass",
    "ProductFamily",
];

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

    /// The price lists a caller asking for `at_epoch_secs` is published: for each Region
    /// the service publishes one in, the ONE version in effect at that instant — the
    /// newest version that took effect on or before it.
    ///
    /// `ListPriceLists$EffectiveDate` is "The date that the Price List file prices are
    /// effective from", so the instant SELECTS a version rather than accumulating the
    /// history: a Region's price list is reported once, at the version a client asking
    /// for that date would download. A Region whose earliest version takes effect later
    /// than the instant has no price list yet, and `region_code` narrows the answer to
    /// one Region.
    pub fn price_lists_in_effect(
        &self,
        service_code: &str,
        currency_code: &str,
        region_code: Option<&str>,
        at_epoch_secs: i64,
    ) -> Vec<&PriceListRecord> {
        let mut in_effect: BTreeMap<Option<String>, &PriceListRecord> = BTreeMap::new();
        for record in self.price_lists.values() {
            if record.service_code != service_code || record.currency_code != currency_code {
                continue;
            }
            if region_code.is_some_and(|region| record.region_code.as_deref() != Some(region)) {
                continue;
            }
            if record.effective_epoch_secs() > at_epoch_secs {
                continue;
            }
            in_effect
                .entry(record.region_code.clone())
                .and_modify(|held| {
                    if held.effective_epoch_secs() < record.effective_epoch_secs() {
                        *held = record;
                    }
                })
                .or_insert(record);
        }
        in_effect.into_values().collect()
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
            // The catalog is read under the lock and the file is rendered OFF it, on a
            // blocking worker: rendering is synchronous CPU work over the whole price
            // list, so holding the state lock — or the async worker — through it would
            // make every other caller of this service wait behind one download.
            let Some(source) = file_source(&state, &path) else {
                return file_response(
                    404,
                    "text/plain",
                    b"no price-list file at this path".to_vec(),
                );
            };
            match ::tokio::task::spawn_blocking(move || render_file_source(&source)).await {
                Ok(Some((content_type, body))) => {
                    let body = if method == ::http::Method::HEAD {
                        Vec::new()
                    } else {
                        body.into_bytes()
                    };
                    file_response(200, content_type, body)
                }
                // The path DOES name a published file, so neither a render that answered
                // nothing (a format no renderer serves) nor one that failed outright is a
                // missing file: both are this service's own fault.
                Ok(None) | Err(_) => file_response(
                    500,
                    "text/plain",
                    b"the price-list file could not be rendered".to_vec(),
                ),
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
/// API can never disagree. The catalog is read under the lock ([`file_source`]) and the
/// render runs with the lock released ([`render_file_source`]).
pub fn render_price_list_file(
    state: &::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>,
    path: &str,
) -> Option<(&'static str, String)> {
    render_file_source(&file_source(state, path)?)
}

/// Everything one price-list file is rendered FROM, copied out of the catalog: the
/// record that describes the file, the format it is published in, and the products it
/// covers.
///
/// Copying is what lets the render run with no lock held. Its size is the price list's
/// own — the catalog decides it, and no request field multiplies it.
struct FileSource {
    record: PriceListRecord,
    file_format: String,
    products: Vec<Product>,
}

/// What the file at `path` is rendered from, or `None` when no price list publishes a
/// file there. Holds the state lock only for the lookup and the copy.
fn file_source(
    state: &::engine::ServiceState<ComAmazonawsPricingAwspricelistserviceState>,
    path: &str,
) -> Option<FileSource> {
    let state = state.lock();
    let (record, file_format) = state.price_lists.values().find_map(|record| {
        record
            .file_paths
            .iter()
            .find(|(_, file_path)| file_path.as_str() == path)
            .map(|(format, _)| (record, format.clone()))
    })?;
    let catalog = state.catalog(&record.service_code)?;
    let products: Vec<Product> = catalog
        .products
        .iter()
        .filter(|product| covers(record, product))
        .cloned()
        .collect();
    Some(FileSource {
        record: record.clone(),
        file_format,
        products,
    })
}

/// The file one [`FileSource`] renders to, as `(content type, body)`. Pure CPU work over
/// data already copied out of the catalog, so it holds no lock.
fn render_file_source(source: &FileSource) -> Option<(&'static str, String)> {
    let products: Vec<&Product> = source.products.iter().collect();
    match source.file_format.as_str() {
        "json" => Some((
            "application/json",
            price_list_file_json(&source.record, &products),
        )),
        "csv" => Some((
            "text/csv",
            price_list_file_csv(&source.record, &products),
        )),
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
///
/// EVERY term type a covered product's offer document carries is copied, keyed by
/// term type and then by SKU the way the offer file nests them — so a product priced
/// both on demand and under a reservation is priced both ways in the file too, and a
/// term type added to the catalog needs no change here. Each term is re-denominated
/// in the record's own currency, so a file never prices in a currency other than the
/// one its `currencyCode` names.
fn price_list_file_json(record: &PriceListRecord, products: &[&Product]) -> String {
    let mut product_blocks = serde_json::Map::new();
    let mut terms: BTreeMap<String, serde_json::Map<String, serde_json::Value>> = BTreeMap::new();
    let percent = currency_percent(&record.currency_code);
    for product in products {
        let Some(offer) = offer_of(product) else {
            continue;
        };
        if let Some(block) = offer.get("product") {
            product_blocks.insert(product.sku.clone(), block.clone());
        }
        for (term_type, by_term_code) in converted_terms(&offer, &record.currency_code, percent) {
            terms
                .entry(term_type)
                .or_default()
                .insert(product.sku.clone(), by_term_code);
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
        "terms": terms,
    })
    .to_string()
}

/// The `v1.0` price-list file in the CSV form the Price List API also publishes it in:
/// the same header [`price_list_file_json`] writes, as leading `name,value` lines, then
/// one ROW per price dimension of every product the price list covers.
///
/// It is the FLAT form of exactly the data the offer document nests — rendered from the
/// same catalog, through the same currency re-denomination — so the two files can never
/// disagree about what a price list holds.
fn price_list_file_csv(record: &PriceListRecord, products: &[&Product]) -> String {
    let percent = currency_percent(&record.currency_code);
    let attribute_names: BTreeSet<&str> = products
        .iter()
        .flat_map(|product| product.attributes.keys().map(String::as_str))
        .collect();
    // Flexible: the file's leading header lines are `name,value` pairs, shorter than the
    // dimension rows that follow them, exactly as the published files are laid out.
    let mut writer = ::csv::WriterBuilder::new()
        .flexible(true)
        .from_writer(Vec::new());
    for (name, value) in [
        ("FormatVersion", FILE_FORMAT_VERSION.to_string()),
        ("Disclaimer", FILE_DISCLAIMER.to_string()),
        ("Publication Date", record.effective_date_text()),
        ("Version", record.version()),
        ("OfferCode", record.service_code.clone()),
    ] {
        let _ = writer.write_record([name, value.as_str()]);
    }
    let header: Vec<&str> = CSV_DIMENSION_COLUMNS
        .iter()
        .copied()
        .chain(attribute_names.iter().copied())
        .collect();
    let _ = writer.write_record(&header);

    for product in products {
        let Some(offer) = offer_of(product) else {
            continue;
        };
        let product_family = pointer_text(&offer, "/product/productFamily");
        for (term_type, by_term_code) in converted_terms(&offer, &record.currency_code, percent) {
            let Some(by_term_code) = by_term_code.as_object() else {
                continue;
            };
            for term in by_term_code.values() {
                let dimensions = term
                    .pointer("/priceDimensions")
                    .and_then(serde_json::Value::as_object);
                for (rate_code, dimension) in dimensions.into_iter().flatten() {
                    let mut row = vec![
                        product.sku.clone(),
                        pointer_text(term, "/offerTermCode"),
                        rate_code.clone(),
                        term_type.clone(),
                        pointer_text(dimension, "/description"),
                        pointer_text(term, "/effectiveDate"),
                        pointer_text(dimension, "/beginRange"),
                        pointer_text(dimension, "/endRange"),
                        pointer_text(dimension, "/unit"),
                        pointer_text(
                            dimension,
                            &format!("/pricePerUnit/{}", record.currency_code),
                        ),
                        record.currency_code.clone(),
                        pointer_text(term, "/termAttributes/LeaseContractLength"),
                        pointer_text(term, "/termAttributes/PurchaseOption"),
                        pointer_text(term, "/termAttributes/OfferingClass"),
                        product_family.clone(),
                    ];
                    // A product that does not carry one of the file's attributes leaves
                    // that column empty, so every row has the same shape as the header.
                    row.extend(attribute_names.iter().map(|name| {
                        product
                            .attributes
                            .get(*name)
                            .cloned()
                            .unwrap_or_default()
                    }));
                    let _ = writer.write_record(&row);
                }
            }
        }
    }
    // The writer writes into a Vec, so neither flushing it nor reading it back as UTF-8
    // can fail; an empty body rather than a panic is the answer if it ever does.
    writer
        .into_inner()
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

/// The string a JSON pointer names, or the empty string when the document does not
/// carry it.
fn pointer_text(document: &serde_json::Value, pointer: &str) -> String {
    document
        .pointer(pointer)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The parsed `aws_v1` offer document of one product, or `None` when it does not parse.
fn offer_of(product: &Product) -> Option<serde_json::Value> {
    serde_json::from_str(&product.offer_json).ok()
}

/// Every term type an offer document prices its product under, re-denominated in
/// `currency_code`: each value is that term type's `terms.<type>` block, keyed by offer
/// term code, exactly as a price-list file nests it.
fn converted_terms(
    offer: &serde_json::Value,
    currency_code: &str,
    percent: u64,
) -> BTreeMap<String, serde_json::Value> {
    offer
        .get("terms")
        .and_then(serde_json::Value::as_object)
        .map(|by_term_type| {
            by_term_type
                .iter()
                .map(|(term_type, by_term_code)| {
                    (
                        term_type.clone(),
                        term_in_currency(by_term_code, currency_code, percent),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One SKU's terms of a single term type, re-denominated in `currency_code`.
///
/// The value is the offer document's own `terms.<type>` block: a map of offer term
/// code to term, each term holding its price dimensions.
///
/// `GetProducts` returns USD-priced offer documents — that is the denomination the
/// Price List Query API answers in — while a price-list FILE is denominated in the one
/// currency its record names. So every price dimension's `pricePerUnit` is re-keyed to
/// that currency and scaled to it, and the rate stated in the dimension's description
/// follows. Everything else is carried through unchanged, including a reserved term's
/// `termAttributes`.
fn term_in_currency(
    by_term_code: &serde_json::Value,
    currency_code: &str,
    percent: u64,
) -> serde_json::Value {
    let mut by_term_code = by_term_code.clone();
    let Some(terms) = by_term_code.as_object_mut() else {
        return by_term_code;
    };
    for term in terms.values_mut() {
        let Some(dimensions) = term
            .pointer_mut("/priceDimensions")
            .and_then(serde_json::Value::as_object_mut)
        else {
            continue;
        };
        for dimension in dimensions.values_mut() {
            let in_usd = dimension
                .pointer("/pricePerUnit/USD")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            let Some(in_usd) = in_usd else { continue };
            let Some(converted) = rate_e10_of(&in_usd).map(|usd| rate_text(scale(usd, percent)))
            else {
                continue;
            };
            // The description states the same rate, so it is restated rather than left
            // claiming the USD figure. It carries the rate exactly once, written the way
            // a description writes it.
            let description = dimension
                .get("description")
                .and_then(serde_json::Value::as_str)
                .map(|text| {
                    text.replace(&in_usd, &converted).replace(
                        &display_rate_text(&in_usd),
                        &display_rate_text(&converted),
                    )
                });
            let Some(object) = dimension.as_object_mut() else {
                continue;
            };
            object.insert(
                "pricePerUnit".to_string(),
                serde_json::json!({ currency_code: converted }),
            );
            if let Some(description) = description {
                object.insert(
                    "description".to_string(),
                    serde_json::Value::String(description),
                );
            }
        }
    }
    by_term_code
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

/// One product to add to the catalog, before its offer document is rendered.
struct ProductDraft {
    /// The product's SKU, unique within its service.
    sku: String,
    /// The `productFamily` the offer document reports.
    product_family: &'static str,
    /// The unit the rate is per (`Hrs`, `GB-Mo`, `Requests`).
    unit: &'static str,
    /// The on-demand rate per `unit`, in 1e-10 USD.
    rate_e10: u64,
    /// Whether this product is also offered under a reservation, which is what gives
    /// its offer document a `terms.Reserved` block beside `terms.OnDemand`.
    reserved: bool,
    /// The product metadata, keyed by attribute name.
    attributes: BTreeMap<String, String>,
}

/// One reserved offer term a reservable product is priced by.
///
/// AWS publishes reserved pricing per lease length, offering class and purchase option,
/// keyed in `terms.Reserved` by the term's own offer term code; the lease lengths the
/// Price List API publishes are `1yr` and `3yr`, and each is offered under the
/// no-upfront, partial-upfront and all-upfront purchase options. `rate_percent` and
/// `upfront_percent` are this emulator's own discount and payment split off the
/// on-demand rate — the real figures are per SKU and are not published as a rule, so
/// they are named here rather than implied.
struct ReservedTermSeed {
    offer_term_code: &'static str,
    /// The lease length in years; `termAttributes.LeaseContractLength` is `<years>yr`.
    lease_years: u64,
    offering_class: &'static str,
    purchase_option: &'static str,
    /// The lease's effective hourly cost as a percentage of the on-demand rate.
    rate_percent: u64,
    /// The share of that lease cost paid as a one-off upfront fee, which is what gives
    /// the term its `Quantity` fee dimension; the remainder is billed per hour.
    upfront_percent: u64,
}

/// The reserved terms every reservable product in this catalog is offered under: one
/// per lease length and purchase option the Price List API publishes reserved pricing
/// for, each keyed by its own offer term code.
const RESERVED_TERMS: &[ReservedTermSeed] = &[
    ReservedTermSeed {
        offer_term_code: "4NA7Y494T4",
        lease_years: 1,
        offering_class: "standard",
        purchase_option: "No Upfront",
        rate_percent: 62,
        upfront_percent: 0,
    },
    ReservedTermSeed {
        offer_term_code: "6QCMYABX3D",
        lease_years: 1,
        offering_class: "standard",
        purchase_option: "Partial Upfront",
        rate_percent: 58,
        upfront_percent: 50,
    },
    ReservedTermSeed {
        offer_term_code: "HU7G6KETJZ",
        lease_years: 1,
        offering_class: "standard",
        purchase_option: "All Upfront",
        rate_percent: 57,
        upfront_percent: 100,
    },
    ReservedTermSeed {
        offer_term_code: "BPH4J8HBKS",
        lease_years: 3,
        offering_class: "standard",
        purchase_option: "No Upfront",
        rate_percent: 44,
        upfront_percent: 0,
    },
    ReservedTermSeed {
        offer_term_code: "38NPMPTW36",
        lease_years: 3,
        offering_class: "standard",
        purchase_option: "Partial Upfront",
        rate_percent: 38,
        upfront_percent: 50,
    },
    ReservedTermSeed {
        offer_term_code: "MZU6U2429S",
        lease_years: 3,
        offering_class: "standard",
        purchase_option: "All Upfront",
        rate_percent: 37,
        upfront_percent: 100,
    },
];

/// The scale AWS publishes a price-list rate at: ten decimal places
/// (`"0.0104000000"`). Every seeded rate is held as an integer count of 1e-10 USD
/// and rendered at exactly that scale, so no rate in this catalog is the result of
/// floating-point arithmetic.
const RATE_SCALE: u64 = 10_000_000_000;

/// A rate held in 1e-10 USD, as an offer document writes it.
fn rate_text(rate_e10: u64) -> String {
    format!("{}.{:010}", rate_e10 / RATE_SCALE, rate_e10 % RATE_SCALE)
}

/// A rate as a price dimension's DESCRIPTION states it: the same number [`rate_text`]
/// writes, without the trailing zeros of its ten-place scale and with at least one
/// decimal place ("0.0052", "0.0"). The service states a rate in a description the way
/// a reader would write it, not at the scale the `pricePerUnit` is published at.
fn display_rate_text(rate_text: &str) -> String {
    let trimmed = rate_text.trim_end_matches('0');
    if trimmed.ends_with('.') {
        format!("{trimmed}0")
    } else {
        trimmed.to_string()
    }
}

/// [`display_rate_text`] of a rate held in 1e-10 USD.
fn display_rate(rate_e10: u64) -> String {
    display_rate_text(&rate_text(rate_e10))
}

/// The integer count of 1e-10 units a rate's text stands for: the inverse of
/// [`rate_text`], for re-denominating a rate already written into a document.
fn rate_e10_of(text: &str) -> Option<u64> {
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    let whole: u64 = whole.parse().ok()?;
    // The scale is fixed at ten places, so a shorter or a longer fraction still reads
    // as the same magnitude.
    let mut digits: String = fraction.chars().take(10).collect();
    while digits.len() < 10 {
        digits.push('0');
    }
    let fraction: u64 = digits.parse().ok()?;
    whole.checked_mul(RATE_SCALE)?.checked_add(fraction)
}

/// How many hundredths of a unit of a currency one USD buys in this catalog.
///
/// USD is 1:1 by definition; the CNY figure is this emulator's own constant — its
/// files say so ([`FILE_DISCLAIMER`]) — not a live exchange rate. A currency with no
/// figure here is carried at parity rather than silently dropped.
const CURRENCY_PER_USD_PERCENT: &[(&str, u64)] = &[("CNY", 720), ("USD", 100)];

/// The percentage of a USD rate a `currency_code` rate is.
fn currency_percent(currency_code: &str) -> u64 {
    CURRENCY_PER_USD_PERCENT
        .iter()
        .find(|(code, _)| *code == currency_code)
        .map_or(100, |(_, percent)| *percent)
}

/// `percent` of `rate_e10`, for a Region, an operating system or a reservation
/// priced as a proportion of a base rate.
fn scale(rate_e10: u64, percent: u64) -> u64 {
    rate_e10.saturating_mul(percent) / 100
}

/// A deterministic, catalog-unique SKU built from the parts that identify a product.
///
/// A Price List SKU is opaque, so what matters is that it is STABLE across reads and
/// names exactly one product.
fn sku(parts: &[&str]) -> String {
    parts
        .iter()
        .flat_map(|part| part.chars())
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_uppercase())
        .collect()
}

/// Collect a product's `(attribute name, value)` pairs into its attribute map.
fn attributes(pairs: Vec<(&str, String)>) -> BTreeMap<String, String> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect()
}

/// The catalog every emulator starts with: every service code the Price List API
/// publishes here with the products that price it, plus one price-list file set per
/// service, Region and published version.
///
/// The catalog itself is IMMUTABLE — no operation this service models creates a service,
/// a product or a price-list file — so it is built ONCE per process
/// ([`SEEDED_CATALOG`]) and each state takes a copy of it, rather than rendering every
/// offer document again on each construction.
fn seeded_catalog() -> ComAmazonawsPricingAwspricelistserviceState {
    let (services, price_lists) = &*SEEDED_CATALOG;
    ComAmazonawsPricingAwspricelistserviceState {
        format_version: DEFAULT_FORMAT_VERSION.to_string(),
        services: services.clone(),
        price_lists: price_lists.clone(),
        // Opened on the first GetPriceListFileUrl, not at construction: opening a
        // listener is asynchronous and needs the shared engine.
        file_server: None,
    }
}

/// The seeded catalog, as `(services, price lists)`, built on first use.
#[allow(clippy::type_complexity)]
static SEEDED_CATALOG: ::std::sync::LazyLock<(
    BTreeMap<String, ServiceCatalog>,
    BTreeMap<String, PriceListRecord>,
)> = ::std::sync::LazyLock::new(build_seeded_catalog);

/// Build the seeded catalog: the services with the products that price them, and the
/// price-list file sets derived from those products.
fn build_seeded_catalog() -> (
    BTreeMap<String, ServiceCatalog>,
    BTreeMap<String, PriceListRecord>,
) {
    let mut services: BTreeMap<String, ServiceCatalog> = BTreeMap::new();
    for (service_code, products) in [
        (EC2_SERVICE_CODE, ec2_products()),
        (S3_SERVICE_CODE, s3_products()),
    ] {
        services.insert(
            service_code.to_string(),
            ServiceCatalog::new(service_code, products),
        );
    }
    for seed in GENERIC_SERVICES {
        services.insert(
            seed.service_code.to_string(),
            ServiceCatalog::new(seed.service_code, generic_products(seed)),
        );
    }

    let price_lists = seeded_price_lists(&services);
    (services, price_lists)
}

/// Build a catalog product from a draft, including the offer document returned for
/// it, so the document can never disagree with the attributes the filters read.
fn product_from_draft(service_code: &str, draft: ProductDraft) -> Product {
    let offer_json = offer_document(service_code, &draft);
    Product {
        sku: draft.sku,
        attributes: draft.attributes,
        offer_json,
    }
}

/// The `aws_v1` product-offer document for one product: the `product` block with its
/// attributes, and every term type that prices it.
fn offer_document(service_code: &str, draft: &ProductDraft) -> String {
    let effective = CATALOG_PUBLICATION_DATE;
    let mut terms = serde_json::Map::new();
    terms.insert(
        "OnDemand".to_string(),
        serde_json::Value::Object(on_demand_term(draft)),
    );
    // Only a product AWS also offers under a reservation carries a Reserved term; one
    // priced on demand only carries none, so the block is absent rather than empty.
    if draft.reserved {
        terms.insert(
            "Reserved".to_string(),
            serde_json::Value::Object(reserved_terms(draft)),
        );
    }

    serde_json::json!({
        "product": {
            "productFamily": draft.product_family,
            "attributes": draft.attributes,
            "sku": draft.sku,
        },
        "serviceCode": service_code,
        "terms": terms,
        "version": version_stamp(effective),
        "publicationDate": effective,
    })
    .to_string()
}

/// The `terms.OnDemand` block for one product: the single on-demand term, keyed by
/// the SKU-derived term code AWS keys it by.
fn on_demand_term(draft: &ProductDraft) -> serde_json::Map<String, serde_json::Value> {
    let sku = draft.sku.as_str();
    let term_key = format!("{sku}.{ON_DEMAND_TERM_CODE}");
    let dimensions = price_dimensions(
        &term_key,
        draft.unit,
        &rate_text(draft.rate_e10),
        &on_demand_description(draft, draft.rate_e10),
    );
    let mut on_demand = serde_json::Map::new();
    on_demand.insert(
        term_key,
        serde_json::json!({
            "sku": sku,
            "offerTermCode": ON_DEMAND_TERM_CODE,
            "effectiveDate": CATALOG_PUBLICATION_DATE,
            "termAttributes": {},
            "priceDimensions": dimensions,
        }),
    );
    on_demand
}

/// The `terms.Reserved` block for one product: one term per lease length, offering
/// class and purchase option the catalog publishes, each keyed by its own offer term
/// code and stating that lease's `LeaseContractLength` in `termAttributes`.
///
/// A term's dimensions are the ones its purchase option actually pays: the usage
/// dimension the lease is billed per hour at, plus — only for an option that pays part
/// or all of the lease up front — the one-off fee dimension, stated as a `Quantity`.
fn reserved_terms(draft: &ProductDraft) -> serde_json::Map<String, serde_json::Value> {
    let sku = draft.sku.as_str();
    let mut reserved = serde_json::Map::new();
    for term in RESERVED_TERMS {
        let term_key = format!("{sku}.{}", term.offer_term_code);
        // The lease's effective hourly cost, of which the upfront share is paid as a
        // fee and the rest per hour of usage.
        let effective_e10 = scale(draft.rate_e10, term.rate_percent);
        let hourly_e10 = scale(effective_e10, 100_u64.saturating_sub(term.upfront_percent));
        let mut dimensions = price_dimensions(
            &term_key,
            draft.unit,
            &rate_text(hourly_e10),
            &reserved_description(draft, hourly_e10),
        );
        if term.upfront_percent > 0 {
            let lease_hours = term.lease_years.saturating_mul(HOURS_PER_LEASE_YEAR);
            let upfront_e10 = scale(
                effective_e10.saturating_mul(lease_hours),
                term.upfront_percent,
            );
            let (rate_code, dimension) = upfront_fee_dimension(&term_key, upfront_e10);
            dimensions.insert(rate_code, dimension);
        }
        reserved.insert(
            term_key,
            serde_json::json!({
                "sku": sku,
                "offerTermCode": term.offer_term_code,
                "effectiveDate": CATALOG_PUBLICATION_DATE,
                "termAttributes": {
                    "LeaseContractLength": format!("{}yr", term.lease_years),
                    "OfferingClass": term.offering_class,
                    "PurchaseOption": term.purchase_option,
                },
                "priceDimensions": dimensions,
            }),
        );
    }
    reserved
}

/// The sentence the Price List API puts on a product's ON-DEMAND usage dimension: the
/// rate, then the usage it prices. A compute instance is priced per instance hour of
/// its operating system and instance type; anything else per unit of the usage type it
/// is billed under.
fn on_demand_description(draft: &ProductDraft, rate_e10: u64) -> String {
    let rate = display_rate(rate_e10);
    match compute_instance(draft) {
        Some((operating_system, instance_type)) => format!(
            "${rate} per On Demand {operating_system} {instance_type} Instance Hour"
        ),
        None => format!("${rate} per {} of {}", draft.unit, usage_of(draft)),
    }
}

/// The sentence the Price List API puts on a RESERVED term's usage dimension: the rate
/// the reservation is billed at, then the usage the reservation applies to.
fn reserved_description(draft: &ProductDraft, rate_e10: u64) -> String {
    let rate = display_rate(rate_e10);
    match compute_instance(draft) {
        Some((operating_system, instance_type)) => format!(
            "USD {rate} per {}, {instance_type} reserved instance applied",
            reserved_platform(operating_system)
        ),
        None => format!("USD {rate} per {} of {}", draft.unit, usage_of(draft)),
    }
}

/// The platform a reserved compute term names: the Price List API writes the Linux
/// platform as the UNIX-family name, and every other operating system under its own.
fn reserved_platform(operating_system: &str) -> String {
    if operating_system == "Linux" {
        "Linux/UNIX (Amazon VPC)".to_string()
    } else {
        format!("{operating_system} (Amazon VPC)")
    }
}

/// The `(operating system, instance type)` a product is a compute instance of, or
/// `None` when it is not priced per instance.
fn compute_instance(draft: &ProductDraft) -> Option<(&str, &str)> {
    let operating_system = draft.attributes.get("operatingSystem")?;
    let instance_type = draft.attributes.get("instanceType")?;
    Some((operating_system.as_str(), instance_type.as_str()))
}

/// The usage a product's rate is billed under: its usage type, or its SKU when it
/// carries none.
fn usage_of(draft: &ProductDraft) -> &str {
    draft
        .attributes
        .get("usagetype")
        .map_or(draft.sku.as_str(), String::as_str)
}

/// The per-unit price dimension of one term, keyed by the rate code AWS derives from
/// the term's key.
fn price_dimensions(
    term_key: &str,
    unit: &str,
    rate: &str,
    description: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let rate_code = format!("{term_key}.{PRICE_DIMENSION_RATE_CODE}");
    let mut dimensions = serde_json::Map::new();
    dimensions.insert(
        rate_code.clone(),
        serde_json::json!({
            "rateCode": rate_code,
            "unit": unit,
            "beginRange": "0",
            "endRange": "Inf",
            "appliesTo": [],
            "description": description,
            "pricePerUnit": { "USD": rate },
        }),
    );
    dimensions
}

/// The one-off fee dimension of a reserved term bought with an upfront payment, as
/// `(rate code, dimension)`. It prices a quantity rather than usage, so it carries no
/// range: the fee is paid once for the whole lease.
fn upfront_fee_dimension(term_key: &str, fee_e10: u64) -> (String, serde_json::Value) {
    let rate_code = format!("{term_key}.{UPFRONT_FEE_RATE_CODE}");
    let dimension = serde_json::json!({
        "rateCode": rate_code,
        "unit": UPFRONT_FEE_UNIT,
        "appliesTo": [],
        "description": UPFRONT_FEE_DESCRIPTION,
        "pricePerUnit": { "USD": rate_text(fee_e10) },
    });
    (rate_code, dimension)
}

/// The price-list file sets the catalog publishes: for every service, one per Region
/// it has products in and per published version.
///
/// DERIVED from the products rather than listed separately, so every price list
/// `ListPriceLists` reports has a file `GetPriceListFileUrl` can resolve and
/// [`render_price_list_file`] can render from this same catalog.
fn seeded_price_lists(
    services: &BTreeMap<String, ServiceCatalog>,
) -> BTreeMap<String, PriceListRecord> {
    let mut price_lists = BTreeMap::new();
    for catalog in services.values() {
        let regions: BTreeSet<&str> = catalog
            .products
            .iter()
            .filter_map(|product| product.attributes.get("regionCode"))
            .map(String::as_str)
            .collect();
        for region_code in regions {
            for effective_date in PRICE_LIST_EFFECTIVE_DATES.iter().copied() {
                let record = price_list_record(
                    &catalog.service_code,
                    region_code,
                    currency_for(region_code),
                    effective_date,
                );
                price_lists.insert(record.arn.clone(), record);
            }
        }
    }
    price_lists
}

/// Build a price-list record: its ARN, the formats it is published in, and the path
/// each of those formats is served at. Every one is derived from the service, Region,
/// currency and effective date, so the ARN `ListPriceLists` reports resolves in
/// `GetPriceListFileUrl` and the URL is the same on every call.
fn price_list_record(
    service_code: &str,
    region_code: &str,
    currency_code: &str,
    effective_date: &str,
) -> PriceListRecord {
    let partition = partition_for(currency_code);
    let version = version_stamp(effective_date);
    let arn = format!(
        // Not `engine::emulator_arn`: a Price List ARN carries neither a Region nor an
        // account id (the modelled pattern is `^arn:…:pricing:::price-list/…$`), which
        // that helper always writes.
        "arn:aws:pricing:::price-list/{partition}/{service_code}/{currency_code}/{version}/{region_code}"
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
                "/offers/v1.0/{partition}/{service_code}/{version}/{region_code}/index.{format}"
            );
            (format.clone(), path)
        })
        .collect();
    PriceListRecord {
        arn,
        service_code: service_code.to_string(),
        region_code: Some(region_code.to_string()),
        currency_code: currency_code.to_string(),
        effective_date: instant_of(effective_date),
        file_formats,
        file_paths,
    }
}

/// The currency the Price List API denominates a Region's files in: `CNY` for the
/// China partition's Regions and `USD` for every other one, which together are
/// exactly [`SUPPORTED_CURRENCY_CODES`].
fn currency_for(region_code: &str) -> &'static str {
    if region_code.starts_with("cn-") {
        "CNY"
    } else {
        "USD"
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

/// The effective dates the catalog publishes a price-list file set for.
///
/// AWS keeps every published version of a price list addressable, so a Region's
/// price list exists in more than one version and `ListPriceLists(EffectiveDate=…)`
/// selects the versions in effect at that instant.
const PRICE_LIST_EFFECTIVE_DATES: &[&str] = &["2023-01-01T00:00:00Z", "2024-06-01T00:00:00Z"];

/// The `locationType` every Region-scoped product in this catalog carries.
const AWS_REGION_LOCATION_TYPE: &str = "AWS Region";

/// The service code of the compute service, whose products are seeded per instance
/// type, Region and operating system.
const EC2_SERVICE_CODE: &str = "AmazonEC2";

/// The `servicename` the compute service's products carry.
const EC2_SERVICE_NAME: &str = "Amazon Elastic Compute Cloud";

/// The service code of the object-storage service, whose products are seeded per
/// storage class and Region.
const S3_SERVICE_CODE: &str = "AmazonS3";

/// The `servicename` the object-storage service's products carry.
const S3_SERVICE_NAME: &str = "Amazon Simple Storage Service";

/// The `licenseModel` of an operating system that needs no license — the value the
/// documented EC2 on-demand filter set keys on for Linux.
const NO_LICENSE_REQUIRED: &str = "No License required";

/// The `licenseModel` of an operating system whose license is in the instance rate.
const LICENSE_INCLUDED: &str = "License included";

// --- Regions ---------------------------------------------------------------

/// One Region the catalog publishes prices for.
struct RegionSeed {
    /// The `regionCode` attribute, and the Region a price list covers.
    code: &'static str,
    /// The `location` attribute: the Region display name the Price List API reports,
    /// which is what `GetAttributeValues(<service>, location)` enumerates.
    location: &'static str,
    /// The prefix AWS puts on a `usagetype` outside us-east-1 (`EU-BoxUsage:…`).
    usage_prefix: &'static str,
    /// This Region's rate as a percentage of the us-east-1 rate, so a Region is
    /// priced differently from the next one.
    rate_percent: u64,
}

impl RegionSeed {
    const fn new(
        code: &'static str,
        location: &'static str,
        usage_prefix: &'static str,
        rate_percent: u64,
    ) -> Self {
        Self {
            code,
            location,
            usage_prefix,
            rate_percent,
        }
    }
}

/// The Regions the catalog publishes prices for, with the display name the Price
/// List API reports for each.
const REGIONS: &[RegionSeed] = &[
    RegionSeed::new("us-east-1", "US East (N. Virginia)", "", 100),
    RegionSeed::new("us-east-2", "US East (Ohio)", "USE2-", 100),
    RegionSeed::new("us-west-1", "US West (N. California)", "USW1-", 118),
    RegionSeed::new("us-west-2", "US West (Oregon)", "USW2-", 100),
    RegionSeed::new("ca-central-1", "Canada (Central)", "CAN1-", 105),
    RegionSeed::new("ca-west-1", "Canada West (Calgary)", "CAW1-", 105),
    RegionSeed::new("eu-west-1", "EU (Ireland)", "EU-", 110),
    RegionSeed::new("eu-west-2", "EU (London)", "EUW2-", 112),
    RegionSeed::new("eu-west-3", "EU (Paris)", "EUW3-", 112),
    RegionSeed::new("eu-central-1", "EU (Frankfurt)", "EUC1-", 115),
    RegionSeed::new("eu-central-2", "EU (Zurich)", "EUC2-", 120),
    RegionSeed::new("eu-north-1", "EU (Stockholm)", "EUN1-", 104),
    RegionSeed::new("eu-south-1", "EU (Milan)", "EUS1-", 112),
    RegionSeed::new("ap-northeast-1", "Asia Pacific (Tokyo)", "APN1-", 125),
    RegionSeed::new("ap-northeast-2", "Asia Pacific (Seoul)", "APN2-", 115),
    RegionSeed::new("ap-northeast-3", "Asia Pacific (Osaka)", "APN3-", 125),
    RegionSeed::new("ap-southeast-1", "Asia Pacific (Singapore)", "APS1-", 120),
    RegionSeed::new("ap-southeast-2", "Asia Pacific (Sydney)", "APS2-", 122),
    RegionSeed::new("ap-southeast-3", "Asia Pacific (Jakarta)", "APS4-", 120),
    RegionSeed::new("ap-south-1", "Asia Pacific (Mumbai)", "APS3-", 102),
    RegionSeed::new("ap-south-2", "Asia Pacific (Hyderabad)", "APS5-", 102),
    RegionSeed::new("ap-east-1", "Asia Pacific (Hong Kong)", "APE1-", 130),
    RegionSeed::new("sa-east-1", "South America (Sao Paulo)", "SAE1-", 155),
    RegionSeed::new("af-south-1", "Africa (Cape Town)", "AFS1-", 130),
    RegionSeed::new("il-central-1", "Israel (Tel Aviv)", "ILC1-", 112),
    RegionSeed::new("me-central-1", "Middle East (UAE)", "MEC1-", 125),
    RegionSeed::new("me-south-1", "Middle East (Bahrain)", "MES1-", 125),
    RegionSeed::new("cn-north-1", "China (Beijing)", "CNN1-", 108),
    RegionSeed::new("cn-northwest-1", "China (Ningxia)", "CNW1-", 108),
];

/// The Regions a service with one usage type per Region is priced in.
const CORE_REGIONS: &[&str] = &["us-east-1", "us-west-2", "eu-west-1", "ap-northeast-1"];

/// The Region `code` names, or `None` when the catalog publishes no prices there.
fn region(code: &str) -> Option<&'static RegionSeed> {
    REGIONS.iter().find(|region| region.code == code)
}

// --- AmazonEC2: compute instances ------------------------------------------

/// One EC2 instance type the catalog prices.
struct InstanceTypeSeed {
    instance_type: &'static str,
    instance_family: &'static str,
    vcpu: &'static str,
    memory: &'static str,
    physical_processor: &'static str,
    current_generation: &'static str,
    /// The us-east-1 Linux on-demand rate per hour, in 1e-10 USD.
    rate_e10: u64,
}

impl InstanceTypeSeed {
    const fn new(
        instance_type: &'static str,
        instance_family: &'static str,
        vcpu: &'static str,
        memory: &'static str,
        physical_processor: &'static str,
        current_generation: &'static str,
        rate_e10: u64,
    ) -> Self {
        Self {
            instance_type,
            instance_family,
            vcpu,
            memory,
            physical_processor,
            current_generation,
            rate_e10,
        }
    }
}

/// The instance types the catalog prices.
///
/// The NAMES are the instance types the Price List API publishes `instanceType` values
/// for, and `vcpu`/`memory` are those types' published specifications; the families
/// cover every `instanceFamily` the API reports for compute (general purpose, compute /
/// memory / storage optimized and GPU), and both current- and previous-generation
/// types. The RATES are this emulator's own, as every file it renders says
/// ([`FILE_DISCLAIMER`]).
const EC2_INSTANCE_TYPES: &[InstanceTypeSeed] = &[
    // --- a1: general purpose, first-generation Graviton -------------------
    InstanceTypeSeed::new("a1.medium", "General purpose", "1", "2 GiB", "AWS Graviton Processor", "Yes", 255_000_000),
    InstanceTypeSeed::new("a1.large", "General purpose", "2", "4 GiB", "AWS Graviton Processor", "Yes", 510_000_000),
    InstanceTypeSeed::new("a1.xlarge", "General purpose", "4", "8 GiB", "AWS Graviton Processor", "Yes", 1_020_000_000),
    InstanceTypeSeed::new("a1.2xlarge", "General purpose", "8", "16 GiB", "AWS Graviton Processor", "Yes", 2_040_000_000),
    InstanceTypeSeed::new("a1.4xlarge", "General purpose", "16", "32 GiB", "AWS Graviton Processor", "Yes", 4_080_000_000),
    InstanceTypeSeed::new("a1.metal", "General purpose", "16", "32 GiB", "AWS Graviton Processor", "Yes", 4_080_000_000),
    // --- t2 / t3 / t3a / t4g: burstable general purpose --------------------
    InstanceTypeSeed::new("t2.nano", "General purpose", "1", "0.5 GiB", "Intel Xeon Family", "No", 58_000_000),
    InstanceTypeSeed::new("t2.micro", "General purpose", "1", "1 GiB", "Intel Xeon Family", "No", 116_000_000),
    InstanceTypeSeed::new("t2.small", "General purpose", "1", "2 GiB", "Intel Xeon Family", "No", 230_000_000),
    InstanceTypeSeed::new("t2.medium", "General purpose", "2", "4 GiB", "Intel Xeon Family", "No", 464_000_000),
    InstanceTypeSeed::new("t2.large", "General purpose", "2", "8 GiB", "Intel Xeon Family", "No", 928_000_000),
    InstanceTypeSeed::new("t2.xlarge", "General purpose", "4", "16 GiB", "Intel Xeon Family", "No", 1_856_000_000),
    InstanceTypeSeed::new("t2.2xlarge", "General purpose", "8", "32 GiB", "Intel Xeon Family", "No", 3_712_000_000),
    InstanceTypeSeed::new("t3.nano", "General purpose", "2", "0.5 GiB", "Intel Skylake E5 2686 v5", "Yes", 52_000_000),
    InstanceTypeSeed::new("t3.micro", "General purpose", "2", "1 GiB", "Intel Skylake E5 2686 v5", "Yes", 104_000_000),
    InstanceTypeSeed::new("t3.small", "General purpose", "2", "2 GiB", "Intel Skylake E5 2686 v5", "Yes", 208_000_000),
    InstanceTypeSeed::new("t3.medium", "General purpose", "2", "4 GiB", "Intel Skylake E5 2686 v5", "Yes", 416_000_000),
    InstanceTypeSeed::new("t3.large", "General purpose", "2", "8 GiB", "Intel Skylake E5 2686 v5", "Yes", 832_000_000),
    InstanceTypeSeed::new("t3.xlarge", "General purpose", "4", "16 GiB", "Intel Skylake E5 2686 v5", "Yes", 1_664_000_000),
    InstanceTypeSeed::new("t3.2xlarge", "General purpose", "8", "32 GiB", "Intel Skylake E5 2686 v5", "Yes", 3_328_000_000),
    InstanceTypeSeed::new("t3a.nano", "General purpose", "2", "0.5 GiB", "AMD EPYC 7571", "Yes", 47_000_000),
    InstanceTypeSeed::new("t3a.micro", "General purpose", "2", "1 GiB", "AMD EPYC 7571", "Yes", 94_000_000),
    InstanceTypeSeed::new("t3a.small", "General purpose", "2", "2 GiB", "AMD EPYC 7571", "Yes", 188_000_000),
    InstanceTypeSeed::new("t3a.medium", "General purpose", "2", "4 GiB", "AMD EPYC 7571", "Yes", 376_000_000),
    InstanceTypeSeed::new("t3a.large", "General purpose", "2", "8 GiB", "AMD EPYC 7571", "Yes", 752_000_000),
    InstanceTypeSeed::new("t3a.xlarge", "General purpose", "4", "16 GiB", "AMD EPYC 7571", "Yes", 1_504_000_000),
    InstanceTypeSeed::new("t3a.2xlarge", "General purpose", "8", "32 GiB", "AMD EPYC 7571", "Yes", 3_008_000_000),
    InstanceTypeSeed::new("t4g.nano", "General purpose", "2", "0.5 GiB", "AWS Graviton2 Processor", "Yes", 42_000_000),
    InstanceTypeSeed::new("t4g.micro", "General purpose", "2", "1 GiB", "AWS Graviton2 Processor", "Yes", 84_000_000),
    InstanceTypeSeed::new("t4g.small", "General purpose", "2", "2 GiB", "AWS Graviton2 Processor", "Yes", 168_000_000),
    InstanceTypeSeed::new("t4g.medium", "General purpose", "2", "4 GiB", "AWS Graviton2 Processor", "Yes", 336_000_000),
    InstanceTypeSeed::new("t4g.large", "General purpose", "2", "8 GiB", "AWS Graviton2 Processor", "Yes", 672_000_000),
    InstanceTypeSeed::new("t4g.xlarge", "General purpose", "4", "16 GiB", "AWS Graviton2 Processor", "Yes", 1_344_000_000),
    InstanceTypeSeed::new("t4g.2xlarge", "General purpose", "8", "32 GiB", "AWS Graviton2 Processor", "Yes", 2_688_000_000),
    // --- m4 / m5 / m5a / m5n / m6g / m6i: general purpose ------------------
    InstanceTypeSeed::new("m4.large", "General purpose", "2", "8 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 1_000_000_000),
    InstanceTypeSeed::new("m4.xlarge", "General purpose", "4", "16 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 2_000_000_000),
    InstanceTypeSeed::new("m4.2xlarge", "General purpose", "8", "32 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 4_000_000_000),
    InstanceTypeSeed::new("m4.4xlarge", "General purpose", "16", "64 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 8_000_000_000),
    InstanceTypeSeed::new("m4.10xlarge", "General purpose", "40", "160 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 20_000_000_000),
    InstanceTypeSeed::new("m4.16xlarge", "General purpose", "64", "256 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 32_000_000_000),
    InstanceTypeSeed::new("m5.large", "General purpose", "2", "8 GiB", "Intel Xeon Platinum 8175", "Yes", 960_000_000),
    InstanceTypeSeed::new("m5.xlarge", "General purpose", "4", "16 GiB", "Intel Xeon Platinum 8175", "Yes", 1_920_000_000),
    InstanceTypeSeed::new("m5.2xlarge", "General purpose", "8", "32 GiB", "Intel Xeon Platinum 8175", "Yes", 3_840_000_000),
    InstanceTypeSeed::new("m5.4xlarge", "General purpose", "16", "64 GiB", "Intel Xeon Platinum 8175", "Yes", 7_680_000_000),
    InstanceTypeSeed::new("m5.8xlarge", "General purpose", "32", "128 GiB", "Intel Xeon Platinum 8175", "Yes", 15_360_000_000),
    InstanceTypeSeed::new("m5.12xlarge", "General purpose", "48", "192 GiB", "Intel Xeon Platinum 8175", "Yes", 23_040_000_000),
    InstanceTypeSeed::new("m5.16xlarge", "General purpose", "64", "256 GiB", "Intel Xeon Platinum 8175", "Yes", 30_720_000_000),
    InstanceTypeSeed::new("m5.24xlarge", "General purpose", "96", "384 GiB", "Intel Xeon Platinum 8175", "Yes", 46_080_000_000),
    InstanceTypeSeed::new("m5.metal", "General purpose", "96", "384 GiB", "Intel Xeon Platinum 8175", "Yes", 46_080_000_000),
    InstanceTypeSeed::new("m5a.large", "General purpose", "2", "8 GiB", "AMD EPYC 7571", "Yes", 860_000_000),
    InstanceTypeSeed::new("m5a.xlarge", "General purpose", "4", "16 GiB", "AMD EPYC 7571", "Yes", 1_720_000_000),
    InstanceTypeSeed::new("m5a.2xlarge", "General purpose", "8", "32 GiB", "AMD EPYC 7571", "Yes", 3_440_000_000),
    InstanceTypeSeed::new("m5a.4xlarge", "General purpose", "16", "64 GiB", "AMD EPYC 7571", "Yes", 6_880_000_000),
    InstanceTypeSeed::new("m5a.8xlarge", "General purpose", "32", "128 GiB", "AMD EPYC 7571", "Yes", 13_760_000_000),
    InstanceTypeSeed::new("m5a.12xlarge", "General purpose", "48", "192 GiB", "AMD EPYC 7571", "Yes", 20_640_000_000),
    InstanceTypeSeed::new("m5a.16xlarge", "General purpose", "64", "256 GiB", "AMD EPYC 7571", "Yes", 27_520_000_000),
    InstanceTypeSeed::new("m5a.24xlarge", "General purpose", "96", "384 GiB", "AMD EPYC 7571", "Yes", 41_280_000_000),
    InstanceTypeSeed::new("m5n.large", "General purpose", "2", "8 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 1_190_000_000),
    InstanceTypeSeed::new("m5n.xlarge", "General purpose", "4", "16 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 2_380_000_000),
    InstanceTypeSeed::new("m5n.2xlarge", "General purpose", "8", "32 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 4_760_000_000),
    InstanceTypeSeed::new("m5n.4xlarge", "General purpose", "16", "64 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 9_520_000_000),
    InstanceTypeSeed::new("m5n.8xlarge", "General purpose", "32", "128 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 19_040_000_000),
    InstanceTypeSeed::new("m5n.12xlarge", "General purpose", "48", "192 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 28_560_000_000),
    InstanceTypeSeed::new("m5n.16xlarge", "General purpose", "64", "256 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 38_080_000_000),
    InstanceTypeSeed::new("m5n.24xlarge", "General purpose", "96", "384 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 57_120_000_000),
    InstanceTypeSeed::new("m6g.medium", "General purpose", "1", "4 GiB", "AWS Graviton2 Processor", "Yes", 385_000_000),
    InstanceTypeSeed::new("m6g.large", "General purpose", "2", "8 GiB", "AWS Graviton2 Processor", "Yes", 770_000_000),
    InstanceTypeSeed::new("m6g.xlarge", "General purpose", "4", "16 GiB", "AWS Graviton2 Processor", "Yes", 1_540_000_000),
    InstanceTypeSeed::new("m6g.2xlarge", "General purpose", "8", "32 GiB", "AWS Graviton2 Processor", "Yes", 3_080_000_000),
    InstanceTypeSeed::new("m6g.4xlarge", "General purpose", "16", "64 GiB", "AWS Graviton2 Processor", "Yes", 6_160_000_000),
    InstanceTypeSeed::new("m6g.8xlarge", "General purpose", "32", "128 GiB", "AWS Graviton2 Processor", "Yes", 12_320_000_000),
    InstanceTypeSeed::new("m6g.12xlarge", "General purpose", "48", "192 GiB", "AWS Graviton2 Processor", "Yes", 18_480_000_000),
    InstanceTypeSeed::new("m6g.16xlarge", "General purpose", "64", "256 GiB", "AWS Graviton2 Processor", "Yes", 24_640_000_000),
    InstanceTypeSeed::new("m6g.metal", "General purpose", "64", "256 GiB", "AWS Graviton2 Processor", "Yes", 24_640_000_000),
    InstanceTypeSeed::new("m6i.large", "General purpose", "2", "8 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 960_000_000),
    InstanceTypeSeed::new("m6i.xlarge", "General purpose", "4", "16 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 1_920_000_000),
    InstanceTypeSeed::new("m6i.2xlarge", "General purpose", "8", "32 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 3_840_000_000),
    InstanceTypeSeed::new("m6i.4xlarge", "General purpose", "16", "64 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 7_680_000_000),
    InstanceTypeSeed::new("m6i.8xlarge", "General purpose", "32", "128 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 15_360_000_000),
    InstanceTypeSeed::new("m6i.12xlarge", "General purpose", "48", "192 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 23_040_000_000),
    InstanceTypeSeed::new("m6i.16xlarge", "General purpose", "64", "256 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 30_720_000_000),
    InstanceTypeSeed::new("m6i.24xlarge", "General purpose", "96", "384 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 46_080_000_000),
    InstanceTypeSeed::new("m6i.32xlarge", "General purpose", "128", "512 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 61_440_000_000),
    InstanceTypeSeed::new("m6i.metal", "General purpose", "128", "512 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 61_440_000_000),
    // --- c1 / c3 / c4 / c5 / c5a / c5n / c6g / c6i: compute optimized ------
    InstanceTypeSeed::new("c1.medium", "Compute optimized", "2", "1.7 GiB", "Intel Xeon Family", "No", 1_300_000_000),
    InstanceTypeSeed::new("c1.xlarge", "Compute optimized", "8", "7 GiB", "Intel Xeon Family", "No", 5_200_000_000),
    InstanceTypeSeed::new("c3.large", "Compute optimized", "2", "3.75 GiB", "Intel Xeon E5-2680 v2 (Ivy Bridge)", "No", 1_050_000_000),
    InstanceTypeSeed::new("c3.xlarge", "Compute optimized", "4", "7.5 GiB", "Intel Xeon E5-2680 v2 (Ivy Bridge)", "No", 2_100_000_000),
    InstanceTypeSeed::new("c3.2xlarge", "Compute optimized", "8", "15 GiB", "Intel Xeon E5-2680 v2 (Ivy Bridge)", "No", 4_200_000_000),
    InstanceTypeSeed::new("c3.4xlarge", "Compute optimized", "16", "30 GiB", "Intel Xeon E5-2680 v2 (Ivy Bridge)", "No", 8_400_000_000),
    InstanceTypeSeed::new("c3.8xlarge", "Compute optimized", "32", "60 GiB", "Intel Xeon E5-2680 v2 (Ivy Bridge)", "No", 16_800_000_000),
    InstanceTypeSeed::new("c4.large", "Compute optimized", "2", "3.75 GiB", "Intel Xeon E5-2666 v3 (Haswell)", "No", 1_000_000_000),
    InstanceTypeSeed::new("c4.xlarge", "Compute optimized", "4", "7.5 GiB", "Intel Xeon E5-2666 v3 (Haswell)", "No", 1_990_000_000),
    InstanceTypeSeed::new("c4.2xlarge", "Compute optimized", "8", "15 GiB", "Intel Xeon E5-2666 v3 (Haswell)", "No", 3_980_000_000),
    InstanceTypeSeed::new("c4.4xlarge", "Compute optimized", "16", "30 GiB", "Intel Xeon E5-2666 v3 (Haswell)", "No", 7_960_000_000),
    InstanceTypeSeed::new("c4.8xlarge", "Compute optimized", "36", "60 GiB", "Intel Xeon E5-2666 v3 (Haswell)", "No", 15_910_000_000),
    InstanceTypeSeed::new("c5.large", "Compute optimized", "2", "4 GiB", "Intel Xeon Platinum 8124M", "Yes", 850_000_000),
    InstanceTypeSeed::new("c5.xlarge", "Compute optimized", "4", "8 GiB", "Intel Xeon Platinum 8124M", "Yes", 1_700_000_000),
    InstanceTypeSeed::new("c5.2xlarge", "Compute optimized", "8", "16 GiB", "Intel Xeon Platinum 8124M", "Yes", 3_400_000_000),
    InstanceTypeSeed::new("c5.4xlarge", "Compute optimized", "16", "32 GiB", "Intel Xeon Platinum 8124M", "Yes", 6_800_000_000),
    InstanceTypeSeed::new("c5.9xlarge", "Compute optimized", "36", "72 GiB", "Intel Xeon Platinum 8124M", "Yes", 15_300_000_000),
    InstanceTypeSeed::new("c5.12xlarge", "Compute optimized", "48", "96 GiB", "Intel Xeon Platinum 8275CL", "Yes", 20_400_000_000),
    InstanceTypeSeed::new("c5.18xlarge", "Compute optimized", "72", "144 GiB", "Intel Xeon Platinum 8124M", "Yes", 30_600_000_000),
    InstanceTypeSeed::new("c5.24xlarge", "Compute optimized", "96", "192 GiB", "Intel Xeon Platinum 8275CL", "Yes", 40_800_000_000),
    InstanceTypeSeed::new("c5.metal", "Compute optimized", "96", "192 GiB", "Intel Xeon Platinum 8275CL", "Yes", 40_800_000_000),
    InstanceTypeSeed::new("c5a.large", "Compute optimized", "2", "4 GiB", "AMD EPYC 7R32", "Yes", 770_000_000),
    InstanceTypeSeed::new("c5a.xlarge", "Compute optimized", "4", "8 GiB", "AMD EPYC 7R32", "Yes", 1_540_000_000),
    InstanceTypeSeed::new("c5a.2xlarge", "Compute optimized", "8", "16 GiB", "AMD EPYC 7R32", "Yes", 3_080_000_000),
    InstanceTypeSeed::new("c5a.4xlarge", "Compute optimized", "16", "32 GiB", "AMD EPYC 7R32", "Yes", 6_160_000_000),
    InstanceTypeSeed::new("c5a.8xlarge", "Compute optimized", "32", "64 GiB", "AMD EPYC 7R32", "Yes", 12_320_000_000),
    InstanceTypeSeed::new("c5a.12xlarge", "Compute optimized", "48", "96 GiB", "AMD EPYC 7R32", "Yes", 18_480_000_000),
    InstanceTypeSeed::new("c5a.16xlarge", "Compute optimized", "64", "128 GiB", "AMD EPYC 7R32", "Yes", 24_640_000_000),
    InstanceTypeSeed::new("c5a.24xlarge", "Compute optimized", "96", "192 GiB", "AMD EPYC 7R32", "Yes", 36_960_000_000),
    InstanceTypeSeed::new("c5n.large", "Compute optimized", "2", "5.25 GiB", "Intel Xeon Platinum 8124M", "Yes", 1_080_000_000),
    InstanceTypeSeed::new("c5n.xlarge", "Compute optimized", "4", "10.5 GiB", "Intel Xeon Platinum 8124M", "Yes", 2_160_000_000),
    InstanceTypeSeed::new("c5n.2xlarge", "Compute optimized", "8", "21 GiB", "Intel Xeon Platinum 8124M", "Yes", 4_320_000_000),
    InstanceTypeSeed::new("c5n.4xlarge", "Compute optimized", "16", "42 GiB", "Intel Xeon Platinum 8124M", "Yes", 8_640_000_000),
    InstanceTypeSeed::new("c5n.9xlarge", "Compute optimized", "36", "96 GiB", "Intel Xeon Platinum 8124M", "Yes", 19_440_000_000),
    InstanceTypeSeed::new("c5n.18xlarge", "Compute optimized", "72", "192 GiB", "Intel Xeon Platinum 8124M", "Yes", 38_880_000_000),
    InstanceTypeSeed::new("c5n.metal", "Compute optimized", "72", "192 GiB", "Intel Xeon Platinum 8124M", "Yes", 38_880_000_000),
    InstanceTypeSeed::new("c6g.medium", "Compute optimized", "1", "2 GiB", "AWS Graviton2 Processor", "Yes", 340_000_000),
    InstanceTypeSeed::new("c6g.large", "Compute optimized", "2", "4 GiB", "AWS Graviton2 Processor", "Yes", 680_000_000),
    InstanceTypeSeed::new("c6g.xlarge", "Compute optimized", "4", "8 GiB", "AWS Graviton2 Processor", "Yes", 1_360_000_000),
    InstanceTypeSeed::new("c6g.2xlarge", "Compute optimized", "8", "16 GiB", "AWS Graviton2 Processor", "Yes", 2_720_000_000),
    InstanceTypeSeed::new("c6g.4xlarge", "Compute optimized", "16", "32 GiB", "AWS Graviton2 Processor", "Yes", 5_440_000_000),
    InstanceTypeSeed::new("c6g.8xlarge", "Compute optimized", "32", "64 GiB", "AWS Graviton2 Processor", "Yes", 10_880_000_000),
    InstanceTypeSeed::new("c6g.12xlarge", "Compute optimized", "48", "96 GiB", "AWS Graviton2 Processor", "Yes", 16_320_000_000),
    InstanceTypeSeed::new("c6g.16xlarge", "Compute optimized", "64", "128 GiB", "AWS Graviton2 Processor", "Yes", 21_760_000_000),
    InstanceTypeSeed::new("c6g.metal", "Compute optimized", "64", "128 GiB", "AWS Graviton2 Processor", "Yes", 21_760_000_000),
    InstanceTypeSeed::new("c6i.large", "Compute optimized", "2", "4 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 850_000_000),
    InstanceTypeSeed::new("c6i.xlarge", "Compute optimized", "4", "8 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 1_700_000_000),
    InstanceTypeSeed::new("c6i.2xlarge", "Compute optimized", "8", "16 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 3_400_000_000),
    InstanceTypeSeed::new("c6i.4xlarge", "Compute optimized", "16", "32 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 6_800_000_000),
    InstanceTypeSeed::new("c6i.8xlarge", "Compute optimized", "32", "64 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 13_600_000_000),
    InstanceTypeSeed::new("c6i.12xlarge", "Compute optimized", "48", "96 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 20_400_000_000),
    InstanceTypeSeed::new("c6i.16xlarge", "Compute optimized", "64", "128 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 27_200_000_000),
    InstanceTypeSeed::new("c6i.24xlarge", "Compute optimized", "96", "192 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 40_800_000_000),
    InstanceTypeSeed::new("c6i.32xlarge", "Compute optimized", "128", "256 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 54_400_000_000),
    InstanceTypeSeed::new("c6i.metal", "Compute optimized", "128", "256 GiB", "Intel Xeon 8375C (Ice Lake)", "Yes", 54_400_000_000),
    // --- r4 / r5 / r6g / x1 / x1e / z1d: memory optimized ------------------
    InstanceTypeSeed::new("r4.large", "Memory optimized", "2", "15.25 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 1_330_000_000),
    InstanceTypeSeed::new("r4.xlarge", "Memory optimized", "4", "30.5 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 2_660_000_000),
    InstanceTypeSeed::new("r4.2xlarge", "Memory optimized", "8", "61 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 5_320_000_000),
    InstanceTypeSeed::new("r4.4xlarge", "Memory optimized", "16", "122 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 10_640_000_000),
    InstanceTypeSeed::new("r4.8xlarge", "Memory optimized", "32", "244 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 21_280_000_000),
    InstanceTypeSeed::new("r4.16xlarge", "Memory optimized", "64", "488 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "No", 42_560_000_000),
    InstanceTypeSeed::new("r5.large", "Memory optimized", "2", "16 GiB", "Intel Xeon Platinum 8175", "Yes", 1_260_000_000),
    InstanceTypeSeed::new("r5.xlarge", "Memory optimized", "4", "32 GiB", "Intel Xeon Platinum 8175", "Yes", 2_520_000_000),
    InstanceTypeSeed::new("r5.2xlarge", "Memory optimized", "8", "64 GiB", "Intel Xeon Platinum 8175", "Yes", 5_040_000_000),
    InstanceTypeSeed::new("r5.4xlarge", "Memory optimized", "16", "128 GiB", "Intel Xeon Platinum 8175", "Yes", 10_080_000_000),
    InstanceTypeSeed::new("r5.8xlarge", "Memory optimized", "32", "256 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 20_160_000_000),
    InstanceTypeSeed::new("r5.12xlarge", "Memory optimized", "48", "384 GiB", "Intel Xeon Platinum 8175", "Yes", 30_240_000_000),
    InstanceTypeSeed::new("r5.16xlarge", "Memory optimized", "64", "512 GiB", "Intel Xeon Platinum 8259 (Cascade Lake)", "Yes", 40_320_000_000),
    InstanceTypeSeed::new("r5.24xlarge", "Memory optimized", "96", "768 GiB", "Intel Xeon Platinum 8175", "Yes", 60_480_000_000),
    InstanceTypeSeed::new("r5.metal", "Memory optimized", "96", "768 GiB", "Intel Xeon Platinum 8175", "Yes", 60_480_000_000),
    InstanceTypeSeed::new("r6g.medium", "Memory optimized", "1", "8 GiB", "AWS Graviton2 Processor", "Yes", 504_000_000),
    InstanceTypeSeed::new("r6g.large", "Memory optimized", "2", "16 GiB", "AWS Graviton2 Processor", "Yes", 1_008_000_000),
    InstanceTypeSeed::new("r6g.xlarge", "Memory optimized", "4", "32 GiB", "AWS Graviton2 Processor", "Yes", 2_016_000_000),
    InstanceTypeSeed::new("r6g.2xlarge", "Memory optimized", "8", "64 GiB", "AWS Graviton2 Processor", "Yes", 4_032_000_000),
    InstanceTypeSeed::new("r6g.4xlarge", "Memory optimized", "16", "128 GiB", "AWS Graviton2 Processor", "Yes", 8_064_000_000),
    InstanceTypeSeed::new("r6g.8xlarge", "Memory optimized", "32", "256 GiB", "AWS Graviton2 Processor", "Yes", 16_128_000_000),
    InstanceTypeSeed::new("r6g.12xlarge", "Memory optimized", "48", "384 GiB", "AWS Graviton2 Processor", "Yes", 24_192_000_000),
    InstanceTypeSeed::new("r6g.16xlarge", "Memory optimized", "64", "512 GiB", "AWS Graviton2 Processor", "Yes", 32_256_000_000),
    InstanceTypeSeed::new("r6g.metal", "Memory optimized", "64", "512 GiB", "AWS Graviton2 Processor", "Yes", 32_256_000_000),
    InstanceTypeSeed::new("x1.16xlarge", "Memory optimized", "64", "976 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 66_690_000_000),
    InstanceTypeSeed::new("x1.32xlarge", "Memory optimized", "128", "1952 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 133_380_000_000),
    InstanceTypeSeed::new("x1e.xlarge", "Memory optimized", "4", "122 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 8_340_000_000),
    InstanceTypeSeed::new("x1e.2xlarge", "Memory optimized", "8", "244 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 16_680_000_000),
    InstanceTypeSeed::new("x1e.4xlarge", "Memory optimized", "16", "488 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 33_360_000_000),
    InstanceTypeSeed::new("x1e.8xlarge", "Memory optimized", "32", "976 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 66_720_000_000),
    InstanceTypeSeed::new("x1e.16xlarge", "Memory optimized", "64", "1952 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 133_440_000_000),
    InstanceTypeSeed::new("x1e.32xlarge", "Memory optimized", "128", "3904 GiB", "Intel Xeon E7-8880 v3 (Haswell)", "No", 266_880_000_000),
    InstanceTypeSeed::new("z1d.large", "Memory optimized", "2", "16 GiB", "Intel Xeon Platinum 8151", "Yes", 1_860_000_000),
    InstanceTypeSeed::new("z1d.xlarge", "Memory optimized", "4", "32 GiB", "Intel Xeon Platinum 8151", "Yes", 3_720_000_000),
    InstanceTypeSeed::new("z1d.2xlarge", "Memory optimized", "8", "64 GiB", "Intel Xeon Platinum 8151", "Yes", 7_440_000_000),
    InstanceTypeSeed::new("z1d.3xlarge", "Memory optimized", "12", "96 GiB", "Intel Xeon Platinum 8151", "Yes", 11_160_000_000),
    InstanceTypeSeed::new("z1d.6xlarge", "Memory optimized", "24", "192 GiB", "Intel Xeon Platinum 8151", "Yes", 22_320_000_000),
    InstanceTypeSeed::new("z1d.12xlarge", "Memory optimized", "48", "384 GiB", "Intel Xeon Platinum 8151", "Yes", 44_640_000_000),
    InstanceTypeSeed::new("z1d.metal", "Memory optimized", "48", "384 GiB", "Intel Xeon Platinum 8151", "Yes", 44_640_000_000),
    // --- d2 / i3: storage optimized ---------------------------------------
    InstanceTypeSeed::new("d2.xlarge", "Storage optimized", "4", "30.5 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 6_900_000_000),
    InstanceTypeSeed::new("d2.2xlarge", "Storage optimized", "8", "61 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 13_800_000_000),
    InstanceTypeSeed::new("d2.4xlarge", "Storage optimized", "16", "122 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 27_600_000_000),
    InstanceTypeSeed::new("d2.8xlarge", "Storage optimized", "36", "244 GiB", "Intel Xeon E5-2676 v3 (Haswell)", "No", 55_200_000_000),
    InstanceTypeSeed::new("i3.large", "Storage optimized", "2", "15.25 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 1_560_000_000),
    InstanceTypeSeed::new("i3.xlarge", "Storage optimized", "4", "30.5 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 3_120_000_000),
    InstanceTypeSeed::new("i3.2xlarge", "Storage optimized", "8", "61 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 6_240_000_000),
    InstanceTypeSeed::new("i3.4xlarge", "Storage optimized", "16", "122 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 12_480_000_000),
    InstanceTypeSeed::new("i3.8xlarge", "Storage optimized", "32", "244 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 24_960_000_000),
    InstanceTypeSeed::new("i3.16xlarge", "Storage optimized", "64", "488 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 49_920_000_000),
    InstanceTypeSeed::new("i3.metal", "Storage optimized", "72", "512 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 49_920_000_000),
    // --- g4dn / p3: GPU ----------------------------------------------------
    InstanceTypeSeed::new("g4dn.xlarge", "GPU instance", "4", "16 GiB", "Intel Xeon Family", "Yes", 5_260_000_000),
    InstanceTypeSeed::new("g4dn.2xlarge", "GPU instance", "8", "32 GiB", "Intel Xeon Family", "Yes", 7_520_000_000),
    InstanceTypeSeed::new("g4dn.4xlarge", "GPU instance", "16", "64 GiB", "Intel Xeon Family", "Yes", 12_040_000_000),
    InstanceTypeSeed::new("g4dn.8xlarge", "GPU instance", "32", "128 GiB", "Intel Xeon Family", "Yes", 21_760_000_000),
    InstanceTypeSeed::new("g4dn.12xlarge", "GPU instance", "48", "192 GiB", "Intel Xeon Family", "Yes", 39_120_000_000),
    InstanceTypeSeed::new("g4dn.16xlarge", "GPU instance", "64", "256 GiB", "Intel Xeon Family", "Yes", 43_520_000_000),
    InstanceTypeSeed::new("p3.2xlarge", "GPU instance", "8", "61 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 30_600_000_000),
    InstanceTypeSeed::new("p3.8xlarge", "GPU instance", "32", "244 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 122_400_000_000),
    InstanceTypeSeed::new("p3.16xlarge", "GPU instance", "64", "488 GiB", "Intel Xeon E5-2686 v4 (Broadwell)", "Yes", 244_800_000_000),
];

/// The instance type `name` names, or `None` when the catalog prices no such type.
fn instance_type_seed(name: &str) -> Option<&'static InstanceTypeSeed> {
    EC2_INSTANCE_TYPES
        .iter()
        .find(|seed| seed.instance_type == name)
}

/// The normalized-unit factor AWS publishes for the SIZES in an instance type's name.
///
/// AWS documents the normalization factors reserved-instance size flexibility is
/// computed in (nano 0.25 through xlarge 8), and every `<n>xlarge` is `n` times the
/// xlarge factor. The table is the published one, keyed by the size in the type name.
const SIZE_FACTORS: &[(&str, &str)] = &[
    ("nano", "0.25"),
    ("micro", "0.5"),
    ("small", "1"),
    ("medium", "2"),
    ("large", "4"),
    ("xlarge", "8"),
];

/// The `normalizationSizeFactor` attribute of an instance type, or `NA` for a size the
/// published factor table does not cover.
fn normalization_size_factor(instance_type: &str) -> String {
    let Some((_, size)) = instance_type.split_once('.') else {
        return "NA".to_string();
    };
    if let Some((_, factor)) = SIZE_FACTORS.iter().find(|(name, _)| *name == size) {
        return (*factor).to_string();
    }
    // `<n>xlarge` is n times the xlarge factor, as the published table continues.
    size.strip_suffix("xlarge")
        .and_then(|multiple| multiple.parse::<u64>().ok())
        .map_or_else(|| "NA".to_string(), |multiple| (multiple * 8).to_string())
}

/// Which instance types the catalog prices in one Region under one operating system.
struct ComputePlan {
    region_code: &'static str,
    operating_system: &'static str,
    /// The `licenseModel` the Price List API reports for this operating system.
    license_model: &'static str,
    /// The `operation` code the usage is billed under.
    operation: &'static str,
    /// This operating system's rate as a percentage of the Linux rate.
    rate_percent: u64,
    /// The instance types this Region prices under this operating system.
    instance_types: PlannedInstanceTypes,
}

impl ComputePlan {
    const fn new(
        region_code: &'static str,
        operating_system: &'static str,
        license_model: &'static str,
        operation: &'static str,
        rate_percent: u64,
        instance_types: PlannedInstanceTypes,
    ) -> Self {
        Self {
            region_code,
            operating_system,
            license_model,
            operation,
            rate_percent,
            instance_types,
        }
    }
}

/// The instance types every Region in the catalog prices: one current-generation size
/// per family the Price List API publishes, plus previous-generation ones, which AWS
/// offers in every commercial Region. Every Region therefore prices them, so a filtered
/// lookup for one of them matches products across the whole catalog — more than one
/// page's worth — and `location` enumerates every Region the catalog publishes.
const COMMON_INSTANCE_TYPES: &[&str] = &[
    "t3.nano",
    "t3.micro",
    "t3.small",
    "t3.medium",
    "t2.micro",
    "m5.large",
    "m5.xlarge",
    "m6g.large",
    "c5.large",
    "c5.xlarge",
    "c6g.large",
    "r5.large",
    "r6g.large",
    "m4.large",
    "c4.large",
    "i3.large",
];

/// Which instance types a [`ComputePlan`] prices.
enum PlannedInstanceTypes {
    /// Every instance type the catalog publishes — what AWS does in the Region it
    /// launches the whole fleet in, and what makes
    /// `GetAttributeValues(AmazonEC2, instanceType)` enumerate the published value set
    /// rather than a sample of it.
    Every,
    /// Exactly the types named, of those the catalog publishes.
    Named(&'static [&'static str]),
}

/// One compute SKU dimension beside the instance type, the Region and the operating
/// system: who the hardware is dedicated to, and whether the hour is capacity in use or
/// capacity a reservation holds unused.
struct CapacitySeed {
    /// The `tenancy` attribute.
    tenancy: &'static str,
    /// The `capacitystatus` attribute.
    capacity_status: &'static str,
    /// The usage this combination is billed under, which is what the `usagetype`
    /// attribute names it by (`BoxUsage:<instance type>`).
    usage_kind: &'static str,
}

/// The tenancy and capacity statuses the Price List API publishes a separate SKU for.
///
/// AWS bills a dedicated instance at the same hourly rate as a shared one (its
/// dedicated charge is per Region, not per instance) and bills the hours a capacity
/// reservation holds unused at the instance's own rate — so these differ in the usage
/// they are billed under, not in the rate.
const CAPACITY_SEEDS: &[CapacitySeed] = &[
    CapacitySeed {
        tenancy: "Shared",
        capacity_status: "Used",
        usage_kind: "BoxUsage",
    },
    CapacitySeed {
        tenancy: "Shared",
        capacity_status: "UnusedCapacityReservation",
        usage_kind: "UnusedBox",
    },
    CapacitySeed {
        tenancy: "Dedicated",
        capacity_status: "Used",
        usage_kind: "DedicatedUsage",
    },
    CapacitySeed {
        tenancy: "Dedicated",
        capacity_status: "UnusedCapacityReservation",
        usage_kind: "UnusedDed",
    },
];

/// The operating systems the launch Region prices the published fleet under, beyond
/// Linux: a licensed platform's rate includes its license, which is the `licenseModel`
/// the Price List API reports for it.
const WINDOWS_INSTANCE_TYPES: &[&str] = &[
    "t2.micro",
    "t2.small",
    "t2.medium",
    "t3.nano",
    "t3.micro",
    "t3.small",
    "t3.medium",
    "t3.large",
    "m4.large",
    "m5.large",
    "m5.xlarge",
    "m5.2xlarge",
    "m6i.large",
    "c4.large",
    "c5.large",
    "c5.xlarge",
    "c6i.large",
    "r4.large",
    "r5.large",
    "r5.xlarge",
    "z1d.large",
];

/// The compute instances the catalog prices. Every Region carries
/// [`COMMON_INSTANCE_TYPES`]; `us-east-1`, where AWS launches the whole fleet, carries
/// every type the catalog publishes.
const EC2_COMPUTE_PLAN: &[ComputePlan] = &[
    ComputePlan::new(
        "us-east-1",
        "Linux",
        NO_LICENSE_REQUIRED,
        "RunInstances",
        100,
        PlannedInstanceTypes::Every,
    ),
    ComputePlan::new("us-east-1", "Windows", LICENSE_INCLUDED, "RunInstances:0002", 200, PlannedInstanceTypes::Named(WINDOWS_INSTANCE_TYPES)),
    ComputePlan::new("us-east-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("us-west-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("us-west-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("us-west-2", "Windows", LICENSE_INCLUDED, "RunInstances:0002", 200, PlannedInstanceTypes::Named(&["m5.large"])),
    ComputePlan::new("ca-central-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ca-west-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-west-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-west-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-west-3", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-central-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-central-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-north-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("eu-south-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-northeast-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-northeast-1", "Windows", LICENSE_INCLUDED, "RunInstances:0002", 200, PlannedInstanceTypes::Named(&["m5.large"])),
    ComputePlan::new("ap-northeast-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-northeast-3", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-southeast-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-southeast-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-southeast-3", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-south-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-south-2", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("ap-east-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("sa-east-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("af-south-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("il-central-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("me-central-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("me-south-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("cn-north-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
    ComputePlan::new("cn-northwest-1", "Linux", NO_LICENSE_REQUIRED, "RunInstances", 100, PlannedInstanceTypes::Named(COMMON_INSTANCE_TYPES)),
];

// --- AmazonEC2: EBS volumes ------------------------------------------------

/// One EBS volume type the catalog prices.
///
/// `volume_type` is the label the Price List API publishes as the `volumeType`
/// attribute — the attribute the model's own `Filter` example names for EC2 volumes.
struct VolumeSeed {
    volume_type: &'static str,
    volume_api_name: &'static str,
    storage_media: &'static str,
    max_iops: &'static str,
    max_throughput: &'static str,
    usagetype: &'static str,
    /// The us-east-1 rate per GB-month, in 1e-10 USD.
    rate_e10: u64,
    /// The provisioned IOPS a volume whose IOPS are billed SEPARATELY from its storage
    /// is priced per, as `(usage type, rate per IOPS-month in 1e-10 USD)`; `None` for a
    /// volume whose IOPS are included in its storage rate.
    provisioned_iops: Option<(&'static str, u64)>,
    /// The provisioned throughput a volume whose throughput is billed separately is
    /// priced per, as `(usage type, rate per MiBps-month in 1e-10 USD)`.
    provisioned_throughput: Option<(&'static str, u64)>,
}

impl VolumeSeed {
    /// The provisioned dimensions this volume type is billed for beside its storage, in
    /// the order the catalog publishes them.
    fn provisioned(&self) -> Vec<(&'static ProvisionedKind, &'static str, u64)> {
        [
            (&IOPS_KIND, self.provisioned_iops),
            (&THROUGHPUT_KIND, self.provisioned_throughput),
        ]
        .into_iter()
        .filter_map(|(kind, provisioned)| {
            provisioned.map(|(usagetype, rate_e10)| (kind, usagetype, rate_e10))
        })
        .collect()
    }
}

/// The EBS volume types the catalog prices, one per volume API name AWS publishes, with
/// the `volumeType` label the Price List API reports for it.
///
/// A volume whose provisioned IOPS or throughput are billed on their own carries those
/// as SEPARATE priced products, as the Price List API publishes them (a `System
/// Operation` / `Provisioned Throughput` product beside the `Storage` one).
const EBS_VOLUMES: &[VolumeSeed] = &[
    VolumeSeed {
        volume_type: "General Purpose",
        volume_api_name: "gp2",
        storage_media: "SSD-backed",
        max_iops: "16000",
        max_throughput: "250 MiB/s",
        usagetype: "EBS:VolumeUsage.gp2",
        rate_e10: 1_000_000_000,
        provisioned_iops: None,
        provisioned_throughput: None,
    },
    VolumeSeed {
        volume_type: "General Purpose",
        volume_api_name: "gp3",
        storage_media: "SSD-backed",
        max_iops: "16000",
        max_throughput: "1000 MiB/s",
        usagetype: "EBS:VolumeUsage.gp3",
        rate_e10: 800_000_000,
        provisioned_iops: Some(("EBS:VolumeP-IOPS.gp3", 50_000_000)),
        provisioned_throughput: Some(("EBS:VolumeP-Throughput.gp3", 400_000_000)),
    },
    VolumeSeed {
        volume_type: "Provisioned IOPS",
        volume_api_name: "io1",
        storage_media: "SSD-backed",
        max_iops: "64000",
        max_throughput: "1000 MiB/s",
        usagetype: "EBS:VolumeUsage.piops",
        rate_e10: 1_250_000_000,
        provisioned_iops: Some(("EBS:VolumeP-IOPS.piops", 650_000_000)),
        provisioned_throughput: None,
    },
    VolumeSeed {
        volume_type: "Provisioned IOPS",
        volume_api_name: "io2",
        storage_media: "SSD-backed",
        max_iops: "64000",
        max_throughput: "1000 MiB/s",
        usagetype: "EBS:VolumeUsage.io2",
        rate_e10: 1_250_000_000,
        provisioned_iops: Some(("EBS:VolumeP-IOPS.io2", 650_000_000)),
        provisioned_throughput: None,
    },
    VolumeSeed {
        volume_type: "Magnetic",
        volume_api_name: "standard",
        storage_media: "HDD-backed",
        max_iops: "40 - 200",
        max_throughput: "40 - 90 MB/sec",
        usagetype: "EBS:VolumeUsage",
        rate_e10: 500_000_000,
        provisioned_iops: None,
        provisioned_throughput: None,
    },
    VolumeSeed {
        volume_type: "Cold HDD",
        volume_api_name: "sc1",
        storage_media: "HDD-backed",
        max_iops: "250",
        max_throughput: "250 MiB/s",
        usagetype: "EBS:VolumeUsage.sc1",
        rate_e10: 150_000_000,
        provisioned_iops: None,
        provisioned_throughput: None,
    },
    VolumeSeed {
        volume_type: "Throughput Optimized HDD",
        volume_api_name: "st1",
        storage_media: "HDD-backed",
        max_iops: "500",
        max_throughput: "500 MiB/s",
        usagetype: "EBS:VolumeUsage.st1",
        rate_e10: 450_000_000,
        provisioned_iops: None,
        provisioned_throughput: None,
    },
];

/// The `productFamily` of a volume's storage product.
const STORAGE_FAMILY: &str = "Storage";

/// What a volume provisions beside its storage, as the Price List API publishes it: its
/// own product family, the usage group reported on that product, and the unit it is
/// priced per.
struct ProvisionedKind {
    product_family: &'static str,
    group: &'static str,
    group_description: &'static str,
    unit: &'static str,
}

/// The product that prices a volume's provisioned IOPS.
const IOPS_KIND: ProvisionedKind = ProvisionedKind {
    product_family: "System Operation",
    group: "EBS IOPS",
    group_description: "IOPS",
    unit: "IOPS-Mo",
};

/// The product that prices a volume's provisioned throughput.
const THROUGHPUT_KIND: ProvisionedKind = ProvisionedKind {
    product_family: "Provisioned Throughput",
    group: "EBS Throughput",
    group_description: "Throughput",
    unit: "MiBps-mo",
};

// --- AmazonS3 ---------------------------------------------------------------

/// One S3 storage class the catalog prices.
struct StorageClassSeed {
    storage_class: &'static str,
    volume_type: &'static str,
    availability: &'static str,
    durability: &'static str,
    usagetype: &'static str,
    /// The us-east-1 rate per GB-month, in 1e-10 USD.
    rate_e10: u64,
}

impl StorageClassSeed {
    const fn new(
        storage_class: &'static str,
        volume_type: &'static str,
        availability: &'static str,
        durability: &'static str,
        usagetype: &'static str,
        rate_e10: u64,
    ) -> Self {
        Self {
            storage_class,
            volume_type,
            availability,
            durability,
            usagetype,
            rate_e10,
        }
    }
}

/// The S3 storage classes the catalog prices.
const S3_STORAGE_CLASSES: &[StorageClassSeed] = &[
    StorageClassSeed::new("General Purpose", "Standard", "99.99%", "99.999999999%", "TimedStorage-ByteHrs", 230_000_000),
    StorageClassSeed::new("Infrequent Access", "Standard - Infrequent Access", "99.9%", "99.999999999%", "TimedStorage-SIA-ByteHrs", 125_000_000),
    StorageClassSeed::new("Archive", "Glacier Flexible Retrieval", "99.99%", "99.999999999%", "TimedStorage-GlacierByteHrs", 36_000_000),
];

// --- Every other service code ----------------------------------------------

/// One service the catalog prices with a single usage type per Region.
///
/// The Price List API publishes a service code for essentially every AWS service, so
/// the catalog has to carry far more than the handful it prices in depth: a client
/// that enumerates service codes, or pages `DescribeServices`, sees the breadth.
struct GenericServiceSeed {
    service_code: &'static str,
    service_name: &'static str,
    product_family: &'static str,
    usagetype: &'static str,
    unit: &'static str,
    /// The us-east-1 rate per `unit`, in 1e-10 USD.
    rate_e10: u64,
}

impl GenericServiceSeed {
    const fn new(
        service_code: &'static str,
        service_name: &'static str,
        product_family: &'static str,
        usagetype: &'static str,
        unit: &'static str,
        rate_e10: u64,
    ) -> Self {
        Self {
            service_code,
            service_name,
            product_family,
            usagetype,
            unit,
            rate_e10,
        }
    }
}

/// The service codes the catalog publishes beyond the two it prices in depth.
const GENERIC_SERVICES: &[GenericServiceSeed] = &[
    GenericServiceSeed::new("AWSAmplify", "AWS Amplify", "Build Minutes", "BuildDuration", "minutes", 100_000_000),
    GenericServiceSeed::new("AWSAppSync", "AWS AppSync", "API Request", "Request", "Requests", 40_000),
    GenericServiceSeed::new("AWSBackup", "AWS Backup", "Storage Snapshot", "Warm-Storage-ByteHrs", "GB-Mo", 500_000_000),
    GenericServiceSeed::new("AWSBudgets", "AWS Budgets", "Budget", "Budget-Day", "Days", 20_000_000),
    GenericServiceSeed::new("AWSCloudFormation", "AWS CloudFormation", "Handler Operation", "HandlerOperation", "Operations", 90_000),
    GenericServiceSeed::new("AWSCloudTrail", "AWS CloudTrail", "Paid Events Recorded", "PaidEventsRecorded", "Events", 20_000),
    GenericServiceSeed::new("AWSCodeArtifact", "AWS CodeArtifact", "Storage", "Storage-ByteHrs", "GB-Mo", 500_000_000),
    GenericServiceSeed::new("AWSCodeCommit", "AWS CodeCommit", "Active User", "ActiveUsers", "Users", 10_000_000_000),
    GenericServiceSeed::new("AWSCodePipeline", "AWS CodePipeline", "Pipeline", "ActivePipeline", "Pipelines", 10_000_000_000),
    GenericServiceSeed::new("AWSConfig", "AWS Config", "Configuration Item Recorded", "ConfigurationItemRecorded", "Items", 30_000_000),
    GenericServiceSeed::new("AWSDataTransfer", "AWS Data Transfer", "Data Transfer", "DataTransfer-Out-Bytes", "GB", 900_000_000),
    GenericServiceSeed::new("AWSDatabaseMigrationSvc", "AWS Database Migration Service", "Database Migration", "dms:t3.medium", "Hrs", 360_000_000),
    GenericServiceSeed::new("AWSDirectConnect", "AWS Direct Connect", "Dedicated Connection", "PortUsage:1G", "Hrs", 3_000_000_000),
    GenericServiceSeed::new("AWSDirectoryService", "AWS Directory Service", "Directory", "DirectoryUsage:Standard", "Hrs", 500_000_000),
    GenericServiceSeed::new("AWSELB", "Elastic Load Balancing", "Load Balancer-Application", "LoadBalancerUsage", "Hrs", 225_000_000),
    GenericServiceSeed::new("AWSEvents", "Amazon EventBridge", "Event", "Event-64K-Chunks", "Events", 100_000),
    GenericServiceSeed::new("AWSGlue", "AWS Glue", "Data Processing Unit", "ETL-DPU-Hour", "DPU-Hour", 4_400_000_000),
    GenericServiceSeed::new("AWSIoT", "AWS IoT", "IoT Message", "Messaging-Messages", "Messages", 10_000),
    GenericServiceSeed::new("AWSKeyManagementService", "AWS Key Management Service", "Key Management", "KMS-Keys", "Keys", 10_000_000_000),
    GenericServiceSeed::new("AWSLambda", "AWS Lambda", "Serverless", "Request", "Requests", 2_000),
    GenericServiceSeed::new("AWSQueueService", "Amazon Simple Queue Service", "API Request", "Requests-Tier1", "Requests", 4_000),
    GenericServiceSeed::new("AWSSecretsManager", "AWS Secrets Manager", "Secret", "Secrets", "Secrets", 4_000_000_000),
    GenericServiceSeed::new("AWSSecurityHub", "AWS Security Hub", "Security Check", "SecurityChecks", "Checks", 100_000),
    GenericServiceSeed::new("AWSShield", "AWS Shield", "DDoS Protection", "ShieldAdvanced", "Months", 30_000_000_000_000),
    GenericServiceSeed::new("AWSStorageGateway", "AWS Storage Gateway", "Storage Gateway", "StorageGateway-ByteHrs", "GB-Mo", 100_000_000),
    GenericServiceSeed::new("AWSSystemsManager", "AWS Systems Manager", "Managed Instance", "AdvancedInstances", "Hrs", 29_580_000),
    GenericServiceSeed::new("AWSTransfer", "AWS Transfer Family", "Transfer Endpoint", "SFTP:Protocol", "Hrs", 3_000_000_000),
    GenericServiceSeed::new("AWSWAF", "AWS WAF", "Web ACL", "WebACLV2", "Months", 50_000_000_000),
    GenericServiceSeed::new("AWSXRay", "AWS X-Ray", "Trace", "XRay-TracesRecorded", "Traces", 50_000),
    GenericServiceSeed::new("AmazonApiGateway", "Amazon API Gateway", "API Calls", "ApiGatewayRequest", "Requests", 35_000),
    GenericServiceSeed::new("AmazonAthena", "Amazon Athena", "Queries", "Athena-DataScannedInTB", "TB", 50_000_000_000),
    GenericServiceSeed::new("AmazonCloudFront", "Amazon CloudFront", "Data Transfer", "DataTransfer-Out-Bytes", "GB", 850_000_000),
    GenericServiceSeed::new("AmazonCloudWatch", "AmazonCloudWatch", "Metric", "CW:MetricMonitorUsage", "Metrics", 3_000_000_000),
    GenericServiceSeed::new("AmazonCognito", "Amazon Cognito", "Monthly Active User", "CognitoMAU", "Users", 55_000_000),
    GenericServiceSeed::new("AmazonConnect", "Amazon Connect", "Agent Minutes", "ConnectAgentMinutes", "Minutes", 180_000_000),
    GenericServiceSeed::new("AmazonDocDB", "Amazon DocumentDB", "Database Instance", "InstanceUsage:db.r5.large", "Hrs", 2_770_000_000),
    GenericServiceSeed::new("AmazonDynamoDB", "Amazon DynamoDB", "Provisioned IOPS", "WriteCapacityUnit-Hrs", "WCU-Hrs", 650_000),
    GenericServiceSeed::new("AmazonECR", "Amazon EC2 Container Registry (ECR)", "Storage", "TimedStorage-ByteHrs", "GB-Mo", 1_000_000_000),
    GenericServiceSeed::new("AmazonECS", "Amazon EC2 Container Service", "Compute", "Fargate-vCPU-Hours:perCPU", "Hrs", 404_800_000),
    GenericServiceSeed::new("AmazonEFS", "Amazon EFS", "Storage", "TimedStorage-ByteHrs", "GB-Mo", 3_000_000_000),
    GenericServiceSeed::new("AmazonEKS", "Amazon Elastic Kubernetes Service", "Compute", "AmazonEKS-Hours:perCluster", "Hrs", 1_000_000_000),
    GenericServiceSeed::new("AmazonES", "Amazon OpenSearch Service", "Elastic Search Instance", "ESInstance:m5.large.search", "Hrs", 1_610_000_000),
    GenericServiceSeed::new("AmazonElastiCache", "Amazon ElastiCache", "Cache Instance", "NodeUsage:cache.t3.micro", "Hrs", 170_000_000),
    GenericServiceSeed::new("AmazonFSx", "Amazon FSx", "Storage", "FSx-SSD-Storage", "GB-Mo", 1_300_000_000),
    GenericServiceSeed::new("AmazonGuardDuty", "Amazon GuardDuty", "Analyzed Event", "PaidEventsAnalyzed", "Events", 100_000),
    GenericServiceSeed::new("AmazonInspector", "Amazon Inspector", "Assessment", "InstanceAssessment", "Assessments", 15_000_000_000),
    GenericServiceSeed::new("AmazonKinesis", "Amazon Kinesis", "Kinesis Streams", "Shard-Hour", "Hrs", 150_000_000),
    GenericServiceSeed::new("AmazonKinesisFirehose", "Amazon Kinesis Firehose", "Kinesis Firehose", "Firehose-DataIngestion", "GB", 290_000_000),
    GenericServiceSeed::new("AmazonMQ", "Amazon MQ", "Broker Instances", "BrokerUsage:mq.t3.micro", "Hrs", 360_000_000),
    GenericServiceSeed::new("AmazonMSK", "Amazon Managed Streaming for Apache Kafka", "Broker Instances", "Kafka.m5.large", "Hrs", 2_100_000_000),
    GenericServiceSeed::new("AmazonNeptune", "Amazon Neptune", "Database Instance", "InstanceUsage:db.r5.large", "Hrs", 3_480_000_000),
    GenericServiceSeed::new("AmazonRDS", "Amazon Relational Database Service", "Database Instance", "InstanceUsage:db.t3.micro", "Hrs", 170_000_000),
    GenericServiceSeed::new("AmazonRedshift", "Amazon Redshift", "Compute Instance", "Node:dc2.large", "Hrs", 2_500_000_000),
    GenericServiceSeed::new("AmazonRoute53", "Amazon Route 53", "DNS Zone", "HostedZone", "HostedZones", 5_000_000_000),
    GenericServiceSeed::new("AmazonSES", "Amazon Simple Email Service", "Sending Email", "Message", "Messages", 1_000),
    GenericServiceSeed::new("AmazonSNS", "Amazon Simple Notification Service", "API Request", "Requests-Tier1", "Requests", 5_000),
    GenericServiceSeed::new("AmazonSageMaker", "Amazon SageMaker", "ML Instance", "ml.m5.large-Hosting", "Hrs", 1_340_000_000),
    GenericServiceSeed::new("AmazonStates", "AWS Step Functions", "State Transition", "StateTransition", "Transitions", 250_000),
    GenericServiceSeed::new("AmazonTimestream", "Amazon Timestream", "Write Requests", "Timestream-WriteBytes", "Writes", 500_000),
    GenericServiceSeed::new("AmazonVPC", "Amazon Virtual Private Cloud", "NAT Gateway", "NatGateway-Hours", "Hrs", 450_000_000),
    GenericServiceSeed::new("AmazonWorkSpaces", "Amazon WorkSpaces", "WorkSpace", "WorkSpace:Standard", "Months", 350_000_000_000),
    GenericServiceSeed::new("ElasticMapReduce", "Amazon Elastic MapReduce", "Elastic Map Reduce Instance", "EMR:m5.xlarge", "Hrs", 480_000_000),
];

// --- Building the products --------------------------------------------------

/// The seeded `AmazonEC2` products: the on-demand compute instances the Price List
/// API prices per instance type, Region and operating system, and the EBS volume
/// products that carry the `volumeType` attribute the model's `Filter` example names.
fn ec2_products() -> Vec<Product> {
    let mut products = Vec::new();
    for plan in EC2_COMPUTE_PLAN {
        let Some(region) = region(plan.region_code) else {
            continue;
        };
        // A plan prices either every type the catalog publishes or exactly the types it
        // names, of those the catalog publishes.
        let instance_types: Vec<&'static InstanceTypeSeed> = match plan.instance_types {
            PlannedInstanceTypes::Every => EC2_INSTANCE_TYPES.iter().collect(),
            PlannedInstanceTypes::Named(names) => names
                .iter()
                .filter_map(|name| instance_type_seed(name))
                .collect(),
        };
        for instance in instance_types {
            for capacity in CAPACITY_SEEDS {
                let draft = ProductDraft {
                    sku: sku(&[
                        instance.instance_type,
                        region.code,
                        plan.operating_system,
                        capacity.tenancy,
                        capacity.capacity_status,
                    ]),
                    product_family: "Compute Instance",
                    unit: "Hrs",
                    rate_e10: scale(
                        scale(instance.rate_e10, region.rate_percent),
                        plan.rate_percent,
                    ),
                    // AWS prices a compute instance both on demand and under a
                    // reservation, so its offer document carries both term types.
                    reserved: true,
                    attributes: attributes(vec![
                        ("capacitystatus", capacity.capacity_status.to_string()),
                        ("currentGeneration", instance.current_generation.to_string()),
                        ("instanceFamily", instance.instance_family.to_string()),
                        ("instanceType", instance.instance_type.to_string()),
                        ("licenseModel", plan.license_model.to_string()),
                        ("location", region.location.to_string()),
                        ("locationType", AWS_REGION_LOCATION_TYPE.to_string()),
                        ("memory", instance.memory.to_string()),
                        (
                            "normalizationSizeFactor",
                            normalization_size_factor(instance.instance_type),
                        ),
                        ("operatingSystem", plan.operating_system.to_string()),
                        ("operation", plan.operation.to_string()),
                        ("physicalProcessor", instance.physical_processor.to_string()),
                        ("preInstalledSw", "NA".to_string()),
                        ("regionCode", region.code.to_string()),
                        ("servicecode", EC2_SERVICE_CODE.to_string()),
                        ("servicename", EC2_SERVICE_NAME.to_string()),
                        ("tenancy", capacity.tenancy.to_string()),
                        (
                            "usagetype",
                            format!(
                                "{}{}:{}",
                                region.usage_prefix, capacity.usage_kind, instance.instance_type
                            ),
                        ),
                        ("vcpu", instance.vcpu.to_string()),
                    ]),
                };
                products.push(product_from_draft(EC2_SERVICE_CODE, draft));
            }
        }
    }
    // AWS publishes the EBS volume types in every Region, and prices a volume's
    // provisioned IOPS and throughput as products of their own where they are billed
    // separately from its storage.
    for region in REGIONS {
        for volume in EBS_VOLUMES {
            products.push(product_from_draft(
                EC2_SERVICE_CODE,
                volume_storage_draft(region, volume),
            ));
            for (kind, usagetype, rate_e10) in volume.provisioned() {
                products.push(product_from_draft(
                    EC2_SERVICE_CODE,
                    volume_provisioned_draft(region, volume, kind, usagetype, rate_e10),
                ));
            }
        }
    }
    products
}

/// The `Storage` product of one EBS volume type in one Region: the volume itself,
/// priced per GB-month.
fn volume_storage_draft(region: &RegionSeed, volume: &VolumeSeed) -> ProductDraft {
    ProductDraft {
        sku: sku(&["EBS", volume.volume_api_name, region.code]),
        product_family: STORAGE_FAMILY,
        unit: "GB-Mo",
        rate_e10: scale(volume.rate_e10, region.rate_percent),
        // A volume is priced on demand only.
        reserved: false,
        attributes: attributes(vec![
            ("location", region.location.to_string()),
            ("locationType", AWS_REGION_LOCATION_TYPE.to_string()),
            ("maxIopsvolume", volume.max_iops.to_string()),
            ("maxThroughputvolume", volume.max_throughput.to_string()),
            ("regionCode", region.code.to_string()),
            ("servicecode", EC2_SERVICE_CODE.to_string()),
            ("servicename", EC2_SERVICE_NAME.to_string()),
            ("storageMedia", volume.storage_media.to_string()),
            (
                "usagetype",
                format!("{}{}", region.usage_prefix, volume.usagetype),
            ),
            ("volumeApiName", volume.volume_api_name.to_string()),
            ("volumeType", volume.volume_type.to_string()),
        ]),
    }
}

/// The product that prices what a volume provisions beside its storage — its IOPS or
/// its throughput — in one Region: a product of its own, in the family and usage group
/// the Price List API reports for it, carrying the same `volumeType` so a filter on the
/// volume type selects everything that volume is billed for.
fn volume_provisioned_draft(
    region: &RegionSeed,
    volume: &VolumeSeed,
    kind: &ProvisionedKind,
    usagetype: &str,
    rate_e10: u64,
) -> ProductDraft {
    ProductDraft {
        sku: sku(&["EBS", usagetype, region.code]),
        product_family: kind.product_family,
        unit: kind.unit,
        rate_e10: scale(rate_e10, region.rate_percent),
        reserved: false,
        attributes: attributes(vec![
            ("group", kind.group.to_string()),
            ("groupDescription", kind.group_description.to_string()),
            ("location", region.location.to_string()),
            ("locationType", AWS_REGION_LOCATION_TYPE.to_string()),
            ("regionCode", region.code.to_string()),
            ("servicecode", EC2_SERVICE_CODE.to_string()),
            ("servicename", EC2_SERVICE_NAME.to_string()),
            (
                "usagetype",
                format!("{}{}", region.usage_prefix, usagetype),
            ),
            ("volumeApiName", volume.volume_api_name.to_string()),
            ("volumeType", volume.volume_type.to_string()),
        ]),
    }
}

/// The seeded `AmazonS3` products: one per storage class and Region.
fn s3_products() -> Vec<Product> {
    let mut products = Vec::new();
    for region_code in CORE_REGIONS.iter().copied() {
        let Some(region) = region(region_code) else {
            continue;
        };
        for class in S3_STORAGE_CLASSES {
            let draft = ProductDraft {
                sku: sku(&["S3", class.usagetype, region.code]),
                product_family: "Storage",
                unit: "GB-Mo",
                rate_e10: scale(class.rate_e10, region.rate_percent),
                reserved: false,
                attributes: attributes(vec![
                    ("availability", class.availability.to_string()),
                    ("durability", class.durability.to_string()),
                    ("location", region.location.to_string()),
                    ("locationType", AWS_REGION_LOCATION_TYPE.to_string()),
                    ("regionCode", region.code.to_string()),
                    ("servicecode", S3_SERVICE_CODE.to_string()),
                    ("servicename", S3_SERVICE_NAME.to_string()),
                    ("storageClass", class.storage_class.to_string()),
                    (
                        "usagetype",
                        format!("{}{}", region.usage_prefix, class.usagetype),
                    ),
                    ("volumeType", class.volume_type.to_string()),
                ]),
            };
            products.push(product_from_draft(S3_SERVICE_CODE, draft));
        }
    }
    products
}

/// The products of a service the catalog prices with one usage type per Region.
fn generic_products(seed: &GenericServiceSeed) -> Vec<Product> {
    let mut products = Vec::new();
    for region_code in CORE_REGIONS.iter().copied() {
        let Some(region) = region(region_code) else {
            continue;
        };
        let draft = ProductDraft {
            sku: sku(&[seed.service_code, region.code]),
            product_family: seed.product_family,
            unit: seed.unit,
            rate_e10: scale(seed.rate_e10, region.rate_percent),
            reserved: false,
            attributes: attributes(vec![
                ("location", region.location.to_string()),
                ("locationType", AWS_REGION_LOCATION_TYPE.to_string()),
                ("regionCode", region.code.to_string()),
                ("servicecode", seed.service_code.to_string()),
                ("servicename", seed.service_name.to_string()),
                (
                    "usagetype",
                    format!("{}{}", region.usage_prefix, seed.usagetype),
                ),
            ]),
        };
        products.push(product_from_draft(seed.service_code, draft));
    }
    products
}
