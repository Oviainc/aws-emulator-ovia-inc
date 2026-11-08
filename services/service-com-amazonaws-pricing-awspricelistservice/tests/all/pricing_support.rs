// SPDX-License-Identifier: CC0-1.0

//! The harness the Price List suites drive the emulator through.
//!
//! Every call goes through the same `MultiServiceRouter::dispatch` the emulator's
//! front door uses, so the suites exercise routing, awsJson1_1 (de)serialization,
//! modelled constraint validation and the handlers together.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_pricing_awspricelistservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Epoch-seconds instants used as `EffectiveDate`. awsJson1_1 carries a timestamp as
/// epoch seconds, so these are the wire form of the dates in the comments.
pub const JAN_1_2022: i64 = 1_640_995_200; // 2022-01-01T00:00:00Z
pub const JAN_1_2024: i64 = 1_704_067_200; // 2024-01-01T00:00:00Z
pub const JUL_1_2024: i64 = 1_719_705_600; // 2024-07-01T00:00:00Z

/// The ARN prefix the model's `PriceListArn` pattern requires
/// (`^arn:[A-Za-z0-9][-.A-Za-z0-9]{0,62}:pricing:::price-list/…$`).
pub const ARN_PREFIX: &str = "arn:aws:pricing:::price-list/";

/// The emulator's front door with only this service mounted.
pub fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The front door and the shared engine behind it, for a test that also reads the
/// catalog directly (the price-list file renderer).
pub fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // These tests exercise the service, not SigV4: the requests below carry a credential
    // scope so the router can name the service, but no real signature.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Call one operation over the wire, returning the HTTP status and the parsed body.
pub async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSPriceListService.{operation}"))
        // The SigV4 credential scope names the service an AWS SDK addressed the request
        // to; it is what the router routes on.
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIDEXAMPLE/20240101/us-east-1/pricing/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0000",
        )
        .body(Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// Call one operation and require it to succeed.
pub async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// Call one operation and require the modelled failure `error_code` with `status`,
/// returning the parsed body so a caller can also assert on its message.
pub async fn err(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    status: u16,
    error_code: &str,
) -> Value {
    let (actual_status, parsed) = call(router, operation, body).await;
    assert_eq!(
        parsed.get("__type").and_then(Value::as_str),
        Some(error_code),
        "case {operation} {body}: expected __type {error_code}, got {parsed}"
    );
    assert_eq!(
        actual_status, status,
        "case {operation} {body}: expected HTTP {status}, got {actual_status} with {parsed}"
    );
    parsed
}

/// The `message` an error body carries.
pub fn error_message(parsed: &Value) -> String {
    parsed
        .get("message")
        .or_else(|| parsed.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One list-shaped output member of a response, or an empty list when absent.
pub fn list_member(response: &Value, member: &str) -> Vec<Value> {
    response
        .get(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The largest page every paginated Price List operation accepts (`MaxResults` 1..100
/// on `DescribeServices`, `GetProducts` and `ListPriceLists`).
pub const MAX_PAGE: i64 = 100;

/// Every item of a paginated listing, by walking it to the end at the largest page the
/// operations accept.
///
/// A single call answers at most one page, so a rule about a result set LARGER than one
/// page (the catalog's instance types, a Region's products) must page to see it whole.
pub async fn all_items(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    items_member: &str,
) -> Vec<Value> {
    let mut body = body.clone();
    body["MaxResults"] = Value::from(MAX_PAGE);
    let mut collected: Vec<Value> = Vec::new();
    let mut pages = 0usize;
    loop {
        let page = ok(router, operation, &body).await;
        collected.extend(list_member(&page, items_member));
        pages += 1;
        let Some(token) = page.get("NextToken").and_then(Value::as_str) else {
            break;
        };
        body["NextToken"] = Value::from(token);
        assert!(
            pages < 200,
            "case {operation}: paging must terminate, still holding a token after \
             {pages} pages"
        );
    }
    collected
}

/// Every offer document a `GetProducts` query matches, paged to the end.
pub async fn all_offer_documents(router: &MultiServiceRouter, body: &Value) -> Vec<Value> {
    all_items(router, "GetProducts", body, "PriceList")
        .await
        .iter()
        .map(|item| {
            let text = item
                .as_str()
                .expect("every PriceList element is a JSON string");
            serde_json::from_str(text).expect("every PriceList element parses as JSON")
        })
        .collect()
}

/// Every value `GetAttributeValues` publishes for an attribute, paged to the end.
pub async fn all_attribute_values(router: &MultiServiceRouter, body: &Value) -> Vec<String> {
    all_items(router, "GetAttributeValues", body, "AttributeValues")
        .await
        .iter()
        .filter_map(|value| value.get("Value").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The service codes a `DescribeServices` response reports.
pub fn service_codes(response: &Value) -> Vec<String> {
    list_member(response, "Services")
        .iter()
        .filter_map(|entry| entry.get("ServiceCode").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The `Services` entry for `service_code`, from a `DescribeServices` response.
pub fn service_entry(response: &Value, service_code: &str) -> Option<Value> {
    list_member(response, "Services")
        .into_iter()
        .find(|entry| entry.get("ServiceCode").and_then(Value::as_str) == Some(service_code))
}

/// The string values of a `GetAttributeValues` response's `AttributeValues`.
pub fn attribute_values(response: &Value) -> Vec<String> {
    list_member(response, "AttributeValues")
        .iter()
        .filter_map(|value| value.get("Value").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The offer documents in a `GetProducts` response: each `PriceList` element is a JSON
/// document carried as a string.
pub fn offer_documents(response: &Value) -> Vec<Value> {
    list_member(response, "PriceList")
        .iter()
        .map(|item| {
            let text = item
                .as_str()
                .expect("every PriceList element is a JSON string");
            serde_json::from_str(text).expect("every PriceList element parses as JSON")
        })
        .collect()
}

/// The SKUs of the offer documents in a `GetProducts` response.
pub fn product_skus(response: &Value) -> Vec<String> {
    offer_documents(response)
        .iter()
        .map(product_sku)
        .collect()
}

/// One offer document's SKU.
pub fn product_sku(document: &Value) -> String {
    document
        .get("product")
        .and_then(|product| product.get("sku"))
        .and_then(Value::as_str)
        .expect("every offer document carries product.sku")
        .to_string()
}

/// One product attribute from an offer document.
pub fn offer_attribute(document: &Value, name: &str) -> Option<String> {
    document
        .get("product")?
        .get("attributes")?
        .get(name)?
        .as_str()
        .map(str::to_string)
}

/// The header row and the dimension rows of a price-list file's CSV form.
///
/// The published layout carries the file header in leading `name,value` lines, so the
/// header row is the record that opens the columns (`SKU` first) and everything after
/// it is one price dimension.
pub fn csv_rows(body: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut reader = ::csv::ReaderBuilder::new()
        .flexible(true)
        .has_headers(false)
        .from_reader(body.as_bytes());
    let records: Vec<Vec<String>> = reader
        .records()
        .filter_map(Result::ok)
        .map(|record| record.iter().map(str::to_string).collect())
        .collect();
    records
        .iter()
        .position(|record| record.first().is_some_and(|first| first == "SKU"))
        .map(|index| (records[index].clone(), records[index + 1..].to_vec()))
        .unwrap_or_default()
}

/// One named column of a CSV row, or `None` when the header does not carry it.
pub fn column_of(header: &[String], row: &[String], column: &str) -> Option<String> {
    let index = header.iter().position(|name| name == column)?;
    row.get(index).cloned()
}

/// Split an `https://host[:port]/path` URL into its authority and its path.
pub fn split_https_url(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://")?;
    let (authority, path) = rest.split_once('/')?;
    Some((authority.to_string(), format!("/{path}")))
}

/// Whether a `host:port` authority names this machine.
pub fn is_loopback(authority: &str) -> bool {
    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "127.0.0.1" || host == "::1" || host == "localhost"
}
