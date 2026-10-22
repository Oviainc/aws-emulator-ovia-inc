Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 57742dc737335ed8f07f6b76d23b07c0a9c73918

# Bootstrap the pricing (AWSPriceListService) emulator

_Filed: 2026-10-21T21:28:21Z_
_Operation: service-com-amazonaws-pricing-awspricelistservice | Class: missing_

> _Note: this issue was identified against the code base at commit `57742dc737335ed8f07f6b76d23b07c0a9c73918`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 5 operations in this crate is an unmodified scaffold (`//! SCAFFOLDED, NOT IMPLEMENTED:` header, returns HTTP 501 `NotImplemented`) and the crate holds zero state, so no Pricing API client works at all. Implement a first, small happy-path slice covering the service's whole read loop — Pricing is a read-only catalog service, so the "create" side is a seeded catalog in state rather than a client-visible CreateX operation.

Wire protocol: `awsJson1_1`, `POST /` with `X-Amz-Target: AWSPriceListService.<OperationName>` (see README.md / OPERATIONS.md).

Happy path, in order:

1. **DescribeServices** — in: `ServiceCode` (optional), `FormatVersion` (optional, len 1..32), `NextToken`, `MaxResults` (1..100, default 100). out: `Services` (list of `Service{ServiceCode (required), AttributeNames}`), `FormatVersion`, `NextToken`. With no `ServiceCode`, return every seeded service; with one (e.g. `AmazonEC2`), return exactly that one service and its real `AttributeNames` from state; for an unknown code return the modeled `NotFoundException` (HTTP 400), not an empty list. Honor `MaxResults` and emit/accept `NextToken` from the stored catalog order (`InvalidNextTokenException` for a token that does not decode).

2. **GetAttributeValues** — in: `ServiceCode` (required), `AttributeName` (required), `NextToken`, `MaxResults` (1..10000, default 100). out: `AttributeValues` (list of `AttributeValue{Value}`), `NextToken`. The values returned must be exactly the distinct values recorded in state for that (service, attribute) pair — e.g. `AmazonEC2`/`instanceType` -> the instance types present in the seeded product set — and must be consistent with what DescribeServices reported in `AttributeNames`. Unknown service or attribute -> `NotFoundException`.

3. **GetProducts** — in: `ServiceCode` (required), `Filters` (list<`Filter{Type (required, TERM_MATCH|EQUALS|CONTAINS|ANY_OF|NONE_OF), Field (required), Value (required)}`>, len 0..50), `FormatVersion`, `NextToken`, `MaxResults` (1..100, default 100). out: `FormatVersion`, `PriceList` (list of JSON strings), `NextToken`. Each returned element must be the stored product-offer JSON document for a product that satisfies ALL supplied filters, matched against the product's stored attributes (at minimum `TERM_MATCH` must actually filter: filtering on `instanceType=t3.micro` must return strictly fewer items than no filter). `MaxResults`/`NextToken` must page over the filtered result set, and `FormatVersion` must echo the stored catalog format version (`aws_v1`).

4. **ListPriceLists** — in: `ServiceCode` (required, len 1..32), `EffectiveDate` (timestamp, required), `CurrencyCode` (required, `^[A-Z]{3}$`), `RegionCode` (optional), `NextToken`, `MaxResults` (1..100). out: `PriceLists` (list of `PriceList{PriceListArn, RegionCode, CurrencyCode, FileFormats}`), `NextToken`. Return the price-list records held in state whose service/currency (and `RegionCode`, when given) match and whose effective date is on or before the requested `EffectiveDate`. The emitted `PriceListArn` must be a stable, stored value matching the modeled pattern `^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/[A-Za-z0-9+_/.-]{1,1023}$` — not a freshly randomized one per call — because step 5 must be able to resolve it. A bad `CurrencyCode` must raise `InvalidParameterException` (HTTP 400).

5. **GetPriceListFileUrl** — in: `PriceListArn` (required, 18..2048, ARN pattern above), `FileFormat` (required, len 1..255). out: `Url`. Look the ARN up in state: unknown ARN -> `ResourceNotFoundException` (HTTP 404); known ARN but a `FileFormat` not in that record's stored `FileFormats` -> `InvalidParameterException`. On success return a deterministic URL derived from the stored record (so the same ARN+format yields the same URL across calls).

NEVER STUBS: every one of these must compute its response from persisted state held in the shared engine (reached via `crate::state::service_state(&*self.engine)`). Hardcoded response bodies, echoing the input back, and random ARNs/URLs with no stored record behind them are failures — the ARN returned by ListPriceLists must resolve in GetPriceListFileUrl, and the attribute values returned by GetAttributeValues must be the ones actually present on the products GetProducts returns.

## State Model

- `PricingState` (in `src/state.rs`), lockable, holding:
  - `format_version: String` — catalog format version, default `"aws_v1"`, returned by DescribeServices/GetProducts.
  - `services: BTreeMap<String /*ServiceCode*/, ServiceCatalog>` — ordered so pagination is stable.
  - `ServiceCatalog { service_code: String, attribute_names: Vec<String>, products: Vec<Product> }`
  - `Product { sku: String, attributes: BTreeMap<String /*attribute name*/, String /*value*/>, offer_json: String /*the full product-offer JSON document returned in GetProducts.PriceList*/ }`
  - `price_lists: BTreeMap<String /*PriceListArn*/, PriceListRecord>`
  - `PriceListRecord { arn: String, service_code: String, region_code: Option<String>, currency_code: String, effective_date: SystemTime, file_formats: Vec<String>, file_urls: BTreeMap<String /*FileFormat*/, String /*Url*/> }`
  - Derived at read time, not stored twice: `GetAttributeValues` = distinct `products[*].attributes[AttributeName]` for the service; `DescribeServices.AttributeNames` = `ServiceCatalog.attribute_names`.
  - Pagination: an opaque `NextToken` encoding (offset, and the query it belongs to) decoded on input; undecodable -> `InvalidNextTokenException`.
  - Seeding: the catalog must be pre-populated at state construction (a couple of services such as `AmazonEC2` and `AmazonS3`, a handful of products each with real attributes, and one or two price-list records per service) since clients cannot create pricing data.

Done = a client script can run steps 1-5 back to back and every response is derived from this state: DescribeServices names a service, GetAttributeValues lists that service's real attribute values, GetProducts returns filtered offer documents, ListPriceLists yields an ARN, and GetPriceListFileUrl resolves that exact ARN to a URL.

The remaining behavioral detail (full filter-type semantics for `CONTAINS`/`ANY_OF`/`NONE_OF`, `ExpiredNextTokenException`, `AccessDeniedException`/`ThrottlingException`, exhaustive validation) stays out of this first slice and will be filed as individual gap issues later.

## Evidence

OPERATIONS.md for this crate lists all 5 bound operations with their input/output members and errors: DescribeServices, GetAttributeValues, GetPriceListFileUrl, GetProducts, ListPriceLists (protocol `awsJson1_1`, target prefix `AWSPriceListService.`). `grep -rn 'SCAFFOLDED, NOT IMPLEMENTED' services/service-com-amazonaws-pricing-awspricelistservice/src/ops` matches all 5 handler files (describe_services.rs, get_attribute_values.rs, get_price_list_file_url.rs, get_products.rs, list_price_lists.rs), each of which returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` -> HTTP 501, so the service holds no state at all. Field/shape details cited above come from the Smithy model: `Service{ServiceCode required, AttributeNames}`, `AttributeValue{Value}`, `PriceList{PriceListArn, RegionCode, CurrencyCode, FileFormats}`, `Filter{Type: TERM_MATCH|EQUALS|CONTAINS|ANY_OF|NONE_OF, Field, Value}` (all required), plus the `PriceListArn` length/pattern constraints and the `CurrencyCode` `^[A-Z]{3}$` pattern. Cross-operation linkage (ListPriceLists -> PriceListArn -> GetPriceListFileUrl; GetAttributeValues `RegionCode` values feeding ListPriceLists) is stated in the model documentation for `PriceList.PriceListArn` and `PriceList.RegionCode`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `pricing` client at it (region us-east-1).
2. `aws pricing describe-services --service-code AmazonEC2` -> expect one Service with ServiceCode=AmazonEC2 and a non-empty AttributeNames list; today returns HTTP 501 NotImplemented.
3. `aws pricing get-attribute-values --service-code AmazonEC2 --attribute-name instanceType` -> expect AttributeValues containing the instance types present in the seeded product catalog.
4. `aws pricing get-products --service-code AmazonEC2 --filters Type=TERM_MATCH,Field=instanceType,Value=t3.micro` -> expect PriceList to contain only offer JSON documents whose instanceType is t3.micro, and strictly fewer items than the unfiltered call; FormatVersion = aws_v1.
5. `aws pricing get-products --service-code AmazonEC2 --max-results 1` twice, passing the returned NextToken the second time -> expect a different product, not the same one repeated.
6. `aws pricing list-price-lists --service-code AmazonEC2 --effective-date 2024-01-01 --currency-code USD` -> expect at least one PriceList entry with a stable PriceListArn and a non-empty FileFormats.
7. `aws pricing get-price-list-file-url --price-list-arn <ARN from previous step> --file-format json` -> expect a Url; repeat the call and expect the identical Url.
8. `aws pricing get-price-list-file-url --price-list-arn arn:aws:pricing:::price-list/does-not-exist --file-format json` -> expect ResourceNotFoundException (HTTP 404).
9. `aws pricing list-price-lists --service-code AmazonEC2 --effective-date 2024-01-01 --currency-code usd` -> expect InvalidParameterException (HTTP 400) for the pattern violation.
10. `aws pricing describe-services --service-code NoSuchService` -> expect NotFoundException (HTTP 400), not an empty Services list.
