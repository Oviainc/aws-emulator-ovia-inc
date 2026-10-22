// SPDX-License-Identifier: CC0-1.0

//! What a managed-thing response carries, checked over a TABLE of inputs rather than
//! over one recorded request:
//!
//! RULE — a read reports every device attribute the stored record holds. Most of those
//! attributes are members of `GetManagedThingResponse` / `ManagedThingSummary` that no
//! request shape declares, and they arrive on the two name/value channels an onboarding
//! request does carry: the `MetaData` pairs, and the named fields of the device bar code
//! supplied as `AuthenticationMaterial`. A value is projected onto the member its name
//! designates only when that member's own generated schema admits it, i.e. only when it
//! satisfies the member's modelled `@length` and `@pattern`.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, status codes and bodies asserted here are the ones a client sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    bar_code_fields, DeviceAttribute, ADVERTISED_PRODUCT_ID, CONNECTOR_DESTINATION_ID,
    CONNECTOR_DEVICE_ID, CONNECTOR_POLICY_ID, DEVICE_SPECIFIC_KEY, INTERNATIONAL_ARTICLE_NUMBER,
    MAC_ADDRESS, PARENT_CONTROLLER_ID, READ_SIDE_ATTRIBUTES, SERIAL_NUMBER,
    UNIVERSAL_PRODUCT_CODE,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for.
const REGION: &str = "us-east-1";

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    /// The one `ManagedThingSummary` a single-record `ListManagedThings` reports.
    fn only_item(&self) -> Option<&Value> {
        self.member("Items")?.as_array()?.first()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request and reduce the answer.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path_and_query: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{REGION}.amazonaws.com{path_and_query}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{REGION}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply { status, body }
}

/// The smallest `CreateManagedThing` request the model accepts, plus `MetaData`.
fn create_request(meta_data: Value) -> Value {
    json!({
        "Role": "CONTROLLER",
        "AuthenticationMaterial": "material-1",
        "AuthenticationMaterialType": "WIFI_SETUP_QR_BAR_CODE",
        "MetaData": meta_data,
    })
}

/// `CreateManagedThing`, asserting the modelled 201 and returning the generated id.
async fn create(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let reply = call(router, "POST", "/managed-things", Some(request)).await;
    assert_eq!(
        reply.status, 201,
        "case {case:?}: expected HTTP 201 from CreateManagedThing, got {} with body {:?}",
        reply.status, reply.body
    );
    reply
        .string("Id")
        .unwrap_or_else(|| panic!("case {case:?}: the create must answer with an Id"))
        .to_string()
}

/// A value of exactly `length` characters that this member's own schema admits (at a legal
/// length), so each attribute gets an input its own modelled `@pattern` accepts.
///
/// The alphabets are tried in order and the first one the member admits at its minimum
/// length is used for every length, so a value built for an out-of-bounds length violates
/// the LENGTH bound alone.
fn admitted_value(attribute: &DeviceAttribute, length: usize) -> String {
    const ALPHABETS: [&str; 2] = ["0123456789", "abcdefghijklmnopqrstuvwxyz0123456789"];
    let (min_len, _) = attribute.length_bounds();
    let alphabet = ALPHABETS
        .into_iter()
        .find(|alphabet| attribute.admits_value(&repeated(alphabet, min_len.max(1))))
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: no test alphabet produces a value this member admits",
                attribute.member
            )
        });
    repeated(alphabet, length)
}

/// `length` characters cycled out of `alphabet`.
fn repeated(alphabet: &str, length: usize) -> String {
    let chars: Vec<char> = alphabet.chars().collect();
    (0..length).map(|index| chars[index % chars.len()]).collect()
}

/// RULE over the whole class: for EVERY read-side attribute, a `MetaData` entry named
/// after the member, carrying a value inside the member's modelled bounds, comes back on
/// that member from `GetManagedThing` — and the metadata entry itself is still reported.
///
/// The table's rows differ along the attribute (nine members, each with its own `@length`
/// bounds and `@pattern`) and along the value's length: the minimum and the maximum the
/// member's own schema publishes. `UniversalProductCode` — the member the issue cited —
/// is one row of nine; the rest are attributes the issue never named.
#[tokio::test]
async fn a_read_reports_every_read_side_attribute_its_metadata_carries() {
    for attribute in READ_SIDE_ATTRIBUTES {
        let (min_len, max_len) = attribute.length_bounds();
        for length in [min_len, max_len] {
            let case = format!("{} @ {length}", attribute.member);
            let value = admitted_value(attribute, length);
            let router = emulator();
            let id = create(
                &router,
                &case,
                create_request(json!({ attribute.member: value.clone() })),
            )
            .await;

            let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
            assert_eq!(
                read.status, 200,
                "case {case:?}: expected HTTP 200 from GetManagedThing, got {} with body {:?}",
                read.status, read.body
            );
            assert_eq!(
                read.string(attribute.member),
                Some(value.as_str()),
                "case {case:?}: expected the stored value {value:?} on {}, got {:?}",
                attribute.member,
                read.member(attribute.member)
            );
            assert_eq!(
                read.member("MetaData"),
                Some(&json!({ attribute.member: value.clone() })),
                "case {case:?}: the metadata entry that carries the attribute is still \
                 reported, got {:?}",
                read.member("MetaData")
            );
        }
    }
}

/// RULE, continued: a metadata value OUTSIDE the member's modelled constraints is not
/// projected — the member stays absent rather than carrying a value the shape forbids —
/// and the metadata entry is still reported verbatim.
///
/// One row per attribute per way of leaving its constraints: one character short of the
/// minimum, one past the maximum, and one character the `@pattern` does not admit.
#[tokio::test]
async fn a_metadata_value_outside_the_modelled_constraints_is_not_projected() {
    for attribute in READ_SIDE_ATTRIBUTES {
        let (min_len, max_len) = attribute.length_bounds();
        let mut rejected: Vec<(String, String)> = Vec::new();
        if min_len > 1 {
            rejected.push((
                "under the minimum length".to_string(),
                admitted_value(attribute, min_len - 1),
            ));
        }
        rejected.push((
            "over the maximum length".to_string(),
            admitted_value(attribute, max_len + 1),
        ));        // A character outside the member's `@pattern`, when the member has one at all.
        // Each candidate is legal in a `MetaData` value (`[a-zA-Z0-9_.,@/:#-]*`) or is
        // skipped, so only the projection rule is under test here.
        let forbidden = ['/', '#', ':'].into_iter().find(|c| {
            let mut value = admitted_value(attribute, min_len.max(1));
            value.pop();
            value.push(*c);
            !attribute.admits_value(&value)
        });
        if let Some(forbidden) = forbidden {
            let mut value = admitted_value(attribute, min_len.max(1));
            value.pop();
            value.push(forbidden);
            rejected.push(("outside the pattern".to_string(), value));
        }

        for (why, value) in rejected {
            let case = format!("{} {why}", attribute.member);
            let router = emulator();
            let id = create(
                &router,
                &case,
                create_request(json!({ attribute.member: value.clone() })),
            )
            .await;

            let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
            assert!(
                read.member(attribute.member).is_none(),
                "case {case:?}: {value:?} is outside the modelled constraints, so {} must be \
                 absent, got {:?}",
                attribute.member,
                read.member(attribute.member)
            );
            assert_eq!(
                read.member("MetaData"),
                Some(&json!({ attribute.member: value.clone() })),
                "case {case:?}: the metadata entry is reported verbatim whether or not it \
                 projects, got {:?}",
                read.member("MetaData")
            );
        }
    }
}

/// RULE over the key spelling: a `MetaData` key names a member by its alphanumerics,
/// case-insensitively, so every separator-and-case variant of one member name reaches the
/// same member — and a key that names no member projects nowhere.
#[tokio::test]
async fn a_metadata_key_names_a_member_by_its_alphanumerics() {
    // (case, metadata key, the value comes back on UniversalProductCode)
    let rows: [(&str, &str, bool); 6] = [
        ("the member name", "UniversalProductCode", true),
        ("lower camel", "universalProductCode", true),
        ("snake case", "universal_product_code", true),
        ("kebab case", "universal-product-code", true),
        ("shouted", "UNIVERSALPRODUCTCODE", true),
        ("another name entirely", "ProductCode", false),
    ];

    for (case, key, projected) in rows {
        let value = admitted_value(&UNIVERSAL_PRODUCT_CODE, UNIVERSAL_PRODUCT_CODE.length_bounds().0);
        let router = emulator();
        let id = create(&router, case, create_request(json!({ key: value.clone() }))).await;
        let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
        let expected = projected.then_some(value.as_str());
        assert_eq!(
            read.string("UniversalProductCode"),
            expected,
            "case {case:?}: expected UniversalProductCode {expected:?}, got {:?}",
            read.member("UniversalProductCode")
        );
    }
}

/// RULE on the LIST side: `ListManagedThings` reports one summary per stored record,
/// carrying the identity members and the read-side attributes `ManagedThingSummary`
/// declares; and the server-side filter on such an attribute selects on the same stored
/// value the summary reports.
#[tokio::test]
async fn a_list_summary_reports_the_read_side_attributes_it_declares() {
    // The `ManagedThingSummary` members that are read-side attributes, with the
    // `ListManagedThings` filter that selects on each — the member's `httpQuery` name as
    // `smithy_query(query="operation", operation="ListManagedThings")` reports it (empty
    // where the model declares no filter for that member).
    let rows: [(&DeviceAttribute, &str); 5] = [
        (&ADVERTISED_PRODUCT_ID, ""),
        (&CONNECTOR_DEVICE_ID, "ConnectorDeviceIdFilter"),
        (&CONNECTOR_POLICY_ID, "ConnectorPolicyIdFilter"),
        (&CONNECTOR_DESTINATION_ID, "ConnectorDestinationIdFilter"),
        (&PARENT_CONTROLLER_ID, "ParentControllerIdentifierFilter"),
    ];

    for (attribute, filter) in rows {
        let case = attribute.member;
        let (min_len, max_len) = attribute.length_bounds();
        let value = admitted_value(attribute, min_len);
        let router = emulator();
        let id = create(
            &router,
            case,
            create_request(json!({ attribute.member: value.clone() })),
        )
        .await;

        let list = call(&router, "GET", "/managed-things", None).await;
        assert_eq!(
            list.status, 200,
            "case {case:?}: expected HTTP 200 from ListManagedThings, got {} with body {:?}",
            list.status, list.body
        );
        let item = list
            .only_item()
            .unwrap_or_else(|| panic!("case {case:?}: one stored record must list one summary"));
        assert_eq!(
            item.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: the summary must identify the stored record, got {:?}",
            item.get("Id")
        );
        assert_eq!(
            item.get(attribute.member).and_then(Value::as_str),
            Some(value.as_str()),
            "case {case:?}: expected the stored value {value:?} on the summary's {}, got {:?}",
            attribute.member,
            item.get(attribute.member)
        );

        if filter.is_empty() {
            continue;
        }
        let selected = call(
            &router,
            "GET",
            &format!("/managed-things?{filter}={value}"),
            None,
        )
        .await;
        assert_eq!(
            selected
                .only_item()
                .and_then(|item| item.get("Id"))
                .and_then(Value::as_str),
            Some(id.as_str()),
            "case {case:?}: a filter on the stored value must select the record, got {:?}",
            selected.body
        );
        let other = admitted_value(attribute, max_len);
        let rejected = call(
            &router,
            "GET",
            &format!("/managed-things?{filter}={other}"),
            None,
        )
        .await;
        assert!(
            rejected.only_item().is_none(),
            "case {case:?}: a filter on another value must select nothing, got {:?}",
            rejected.body
        );
    }
}

/// RULE: a modelled error answer carries its modelled members — `ResourceNotFoundException`
/// declares `Message`, `ResourceId` and `ResourceType`, so a 404 from this service names
/// what was not found rather than answering an empty envelope.
#[tokio::test]
async fn a_modelled_error_answer_carries_its_modelled_members() {
    let router = emulator();
    let missing = call(&router, "GET", "/managed-things/no-such-thing", None).await;
    assert_eq!(
        missing.status, 404,
        "case \"absent\": expected HTTP 404, got {} with body {:?}",
        missing.status, missing.body
    );
    assert!(
        missing.string("Message").is_some_and(|m| !m.is_empty()),
        "case \"absent\": the 404 must carry a Message, got body {:?}",
        missing.body
    );
    assert_eq!(
        missing.string("ResourceId"),
        Some("no-such-thing"),
        "case \"absent\": the 404 names the resource asked for, got body {:?}",
        missing.body
    );

    // A deleted record: the read answers the modelled 404, which names what was asked for.
    let id = create(&router, "deleted", create_request(json!({"room": "living"}))).await;
    let deleted = call(&router, "DELETE", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        deleted.status, 204,
        "case \"deleted\": expected HTTP 204 from the delete, got {}",
        deleted.status
    );
    let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        read.status, 404,
        "case \"deleted\": expected HTTP 404, got {} with body {:?}",
        read.status, read.body
    );
    assert!(
        read.string("Message").is_some_and(|m| !m.is_empty()),
        "case \"deleted\": the 404 must carry a Message, got body {:?}",
        read.body
    );
    assert_eq!(
        read.string("ResourceId"),
        Some(id.as_str()),
        "case \"deleted\": the 404 names the resource asked for, got body {:?}",
        read.body
    );
}

/// RULE on the bar-code channel: the device attributes also arrive in the named fields of
/// the bar code the create call supplies as `AuthenticationMaterial`, in either of the two
/// payload shapes that carry names — a JSON object and `Name:value` fields separated by
/// `;` — under the member's name or its standard abbreviation; and `MetaData` wins where
/// both name the same attribute.
#[tokio::test]
async fn a_device_bar_codes_named_fields_reach_the_attributes_they_name() {
    let upc = admitted_value(&UNIVERSAL_PRODUCT_CODE, UNIVERSAL_PRODUCT_CODE.length_bounds().0);
    let serial = admitted_value(&SERIAL_NUMBER, 13);

    // (case, the bar code, the UniversalProductCode it names, the SerialNumber it names)
    let rows: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
        (
            "a JSON object naming members",
            format!("{{\"SerialNumber\":\"{serial}\",\"UniversalProductCode\":\"{upc}\"}}"),
            Some(upc.as_str()),
            Some(serial.as_str()),
        ),
        (
            "a JSON object naming abbreviations",
            format!("{{\"SN\":\"{serial}\",\"UPC\":\"{upc}\"}}"),
            Some(upc.as_str()),
            Some(serial.as_str()),
        ),
        (
            "delimited fields naming members",
            format!("SerialNumber:{serial};UniversalProductCode:{upc}"),
            Some(upc.as_str()),
            Some(serial.as_str()),
        ),
        (
            "delimited fields naming abbreviations",
            format!("SN:{serial};UPC:{upc};"),
            Some(upc.as_str()),
            Some(serial.as_str()),
        ),
        (
            // A payload that names nothing: a digit-only Z-Wave SmartStart code.
            "an unnamed payload",
            "900123456789012345678901234567890123456789".to_string(),
            None,
            None,
        ),
        (
            // A named field whose value is outside the member's modelled constraints.
            "a field outside the constraints",
            "UPC:12345".to_string(),
            None,
            None,
        ),
    ];

    for (case, material, expected_upc, expected_serial) in rows {
        let router = emulator();
        let id = create(
            &router,
            case,
            json!({
                "Role": "DEVICE",
                "AuthenticationMaterial": material,
                "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
            }),
        )
        .await;
        let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
        assert_eq!(
            read.string("UniversalProductCode"),
            expected_upc,
            "case {case:?}: expected UniversalProductCode {expected_upc:?}, got {:?}",
            read.member("UniversalProductCode")
        );
        assert_eq!(
            read.string("SerialNumber"),
            expected_serial,
            "case {case:?}: expected SerialNumber {expected_serial:?}, got {:?}",
            read.member("SerialNumber")
        );
        // The material itself is `@sensitive` and no output shape declares it.
        assert!(
            read.member("AuthenticationMaterial").is_none(),
            "case {case:?}: the bar code itself is never reported, got {:?}",
            read.member("AuthenticationMaterial")
        );
    }

    // A `MetaData` entry and a bar-code field naming the same attribute: the metadata
    // entry is what the caller set on THIS resource, so it wins. `other` is the same
    // modelled length as `upc` and a different value.
    let other = {
        let mut rotated = upc.clone();
        let first = rotated.remove(0);
        rotated.push(first);
        rotated
    };
    assert_ne!(other, upc, "the two product codes must differ");
    let router = emulator();
    let id = create(
        &router,
        "metadata wins",
        json!({
            "Role": "DEVICE",
            "AuthenticationMaterial": format!("UPC:{upc}"),
            "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
            "MetaData": {"UniversalProductCode": other.clone()},
        }),
    )
    .await;
    let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        read.string("UniversalProductCode"),
        Some(other.as_str()),
        "case \"metadata wins\": expected the metadata value {other:?}, got {:?}",
        read.member("UniversalProductCode")
    );

    // A serial number the request names wins over one the bar code carries.
    let named = admitted_value(&SERIAL_NUMBER, 7);
    let router = emulator();
    let id = create(
        &router,
        "a named serial number wins",
        json!({
            "Role": "DEVICE",
            "AuthenticationMaterial": format!("SN:{serial}"),
            "AuthenticationMaterialType": "ZWAVE_QR_BAR_CODE",
            "SerialNumber": named.clone(),
        }),
    )
    .await;
    let read = call(&router, "GET", &format!("/managed-things/{id}"), None).await;
    assert_eq!(
        read.string("SerialNumber"),
        Some(named.as_str()),
        "case \"a named serial number wins\": expected {named:?}, got {:?}",
        read.member("SerialNumber")
    );
}

/// One row of [`a_bar_code_names_fields_only_in_the_two_named_payload_shapes`]: the
/// case's name, the bar-code payload, and the name/value fields it is expected to name.
type BarCodeRow = (
    &'static str,
    &'static str,
    &'static [(&'static str, &'static str)],
);

/// The bar-code reader itself: which payload shapes name fields, and which name none.
#[test]
fn a_bar_code_names_fields_only_in_the_two_named_payload_shapes() {
    // (case, material, the fields it names)
    let rows: [BarCodeRow; 7] = [
        (
            "a JSON object",
            "{\"UPC\":\"805730036828\",\"SN\":\"5645029729099\"}",
            &[("UPC", "805730036828"), ("SN", "5645029729099")],
        ),
        (
            "a JSON object with a number",
            "{\"TimeoutInMinutes\":7}",
            &[("TimeoutInMinutes", "7")],
        ),
        (
            // A composite under a name is not one attribute value.
            "a JSON object with a nested object",
            "{\"Nested\":{\"a\":1},\"Flat\":\"v\"}",
            &[("Flat", "v")],
        ),
        (
            "delimited fields",
            "UPC:805730036828;SN:5645029729099;;",
            &[("UPC", "805730036828"), ("SN", "5645029729099")],
        ),
        (
            // A DPP bootstrapping URI: its labels name no modelled attribute, but the
            // reader still reads them as fields rather than failing.
            "a DPP URI",
            "DPP:C:81/1;M:010203040506;;",
            &[("DPP", "C:81/1"), ("M", "010203040506")],
        ),
        ("a digit-only code", "9001234567890123456789", &[]),
        ("malformed JSON", "{\"UPC\":", &[]),
    ];

    for (case, material, expected) in rows {
        let fields = bar_code_fields(material);
        let mut got: Vec<(String, String)> = fields.into_iter().collect();
        got.sort();
        let mut want: Vec<(String, String)> = expected
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        want.sort();
        assert_eq!(
            got, want,
            "case {case:?}: expected the bar code to name {want:?}, got {got:?}"
        );
    }
}

/// The attribute table itself: every entry names a member `GetManagedThingResponse`
/// really declares, each member's `@length` bounds come back from its generated schema as
/// the model publishes them, and `admits_value` is exactly "inside those bounds and inside
/// the member's pattern". Pins the table against silent drift in one place rather than
/// re-stating the bounds in every case above.
#[test]
fn the_read_side_attribute_table_names_modelled_members_with_their_published_bounds() {
    // (member, min, max) as `smithy_query(query="operation", operation="GetManagedThing")`
    // publishes the target shape of each read-side member of `GetManagedThingResponse`.
    // These literals are the EXPECTATION, not the implementation: the bounds the code uses
    // are read from the generated member schema, and this case is what checks the two
    // agree.
    let published: [(&DeviceAttribute, &str, usize, usize); 9] = [
        (&ADVERTISED_PRODUCT_ID, "AdvertisedProductId", 5, 5),
        (&UNIVERSAL_PRODUCT_CODE, "UniversalProductCode", 12, 12),
        (
            &INTERNATIONAL_ARTICLE_NUMBER,
            "InternationalArticleNumber",
            8,
            13,
        ),
        (&CONNECTOR_POLICY_ID, "ConnectorPolicyId", 1, 64),
        (&CONNECTOR_DESTINATION_ID, "ConnectorDestinationId", 1, 64),
        (&CONNECTOR_DEVICE_ID, "ConnectorDeviceId", 1, 256),
        (&DEVICE_SPECIFIC_KEY, "DeviceSpecificKey", 1, 128),
        (&MAC_ADDRESS, "MacAddress", 1, 128),
        (&PARENT_CONTROLLER_ID, "ParentControllerId", 1, 64),
    ];
    assert_eq!(
        published.len(),
        READ_SIDE_ATTRIBUTES.len(),
        "the table must hold exactly the read-side members of GetManagedThingResponse, got {:?}",
        READ_SIDE_ATTRIBUTES
            .iter()
            .map(|attribute| attribute.member)
            .collect::<Vec<_>>()
    );

    // `SerialNumber` is reported by the same shape, so its schema must resolve too even
    // though it is not a read-side-only member.
    for attribute in READ_SIDE_ATTRIBUTES.iter().copied().chain([&SERIAL_NUMBER]) {
        assert!(
            attribute.schema().is_some(),
            "case {:?}: the entry must name a member GetManagedThingResponse declares",
            attribute.member
        );
    }

    for (attribute, member, min, max) in published {
        assert_eq!(
            (attribute.member, attribute.length_bounds()),
            (member, (min, max)),
            "case {member:?}: the member's schema must publish these modelled bounds"
        );
        // Each published bound is admitted; one character past either is not.
        assert!(
            attribute.admits_value(&admitted_value(attribute, min)),
            "case {member:?}: the minimum length {min} must be admitted"
        );
        assert!(
            attribute.admits_value(&admitted_value(attribute, max)),
            "case {member:?}: the maximum length {max} must be admitted"
        );
        assert!(
            !attribute.admits_value(&admitted_value(attribute, max + 1)),
            "case {member:?}: one character past the maximum {max} must be refused"
        );
        if min > 1 {
            assert!(
                !attribute.admits_value(&admitted_value(attribute, min - 1)),
                "case {member:?}: one character short of the minimum {min} must be refused"
            );
        }
        assert!(
            !attribute.admits_value(""),
            "case {member:?}: an empty value is shorter than every modelled minimum"
        );
    }
}
