// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestParsing`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.TestParsing`) through the same router the emulator boots, with the
//! Amazon S3 service mounted beside b2bi on one engine — `TestParsing` reads the EDI
//! document it parses from the S3 object `inputFile` names, by calling S3 in process,
//! exactly as the assembled emulator does.
//!
//! The rule under test: `TestParsing` answers the EDI interchange held by the stored
//! object `inputFile` names, parsed into a document in the requested `fileFormat` whose
//! tree is the X12 envelope nest — interchanges, holding functional groups, holding
//! transaction sets, holding each transaction's own segments — with every element keyed by
//! its segment and position and carrying the value the interchange holds, read with the
//! delimiters the interchange's own control header declares; content that is not an EDI
//! interchange is refused with the modelled `ValidationException` and an `inputFile` that
//! names no stored object with the modelled `ResourceNotFoundException`.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what it generalises
//! over: the delimiter set, the sender and receiver, the transaction set, the number of
//! functional groups and transaction sets, composite elements, absent elements, the
//! requested `fileFormat` and the configured split and validation options.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use smithy_server_runtime::MultiServiceRouter;

// The wire harness this test target shares: a signed awsJson1_0 call and the `__type` of
// an error body.
use crate::onboarding_loop::{call, error_code, SIGNING_REGION};

/// The bucket every document below is staged in.
const BUCKET: &str = "b2bi-test-parsing";

/// A router with the b2bi service and the Amazon S3 service on one shared engine — what
/// `build_router` assembles, narrowed to the services under test.
fn parsing_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(service_com_amazonaws_b2bi_b2bi::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// One path-style request to the emulator's S3 front door: the bucket and the key live in
/// the URI and the body is the object's own bytes, so this is a raw request rather than an
/// `x-amz-target` call. The credential scope names `s3`, which is what the router claims
/// on.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{SIGNING_REGION}.amazonaws.com{path}"))
        .header("host", format!("s3.{SIGNING_REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{SIGNING_REGION}/s3/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        response.body().as_ref().to_vec(),
    )
}

/// Create [`BUCKET`], so the objects below have somewhere to live.
///
/// The create names its `LocationConstraint`: this suite signs for [`SIGNING_REGION`], and
/// S3 serves a create that names none only at the endpoint the unspecified constraint
/// stands for (us-east-1).
async fn create_bucket(router: &MultiServiceRouter) {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{BUCKET}"),
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{SIGNING_REGION}</LocationConstraint>\
             </CreateBucketConfiguration>"
        )
        .as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "the bucket must be created first, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
}

/// Store `content` at `key` in [`BUCKET`] and answer the `inputFile` that names it.
async fn stage(router: &MultiServiceRouter, key: &str, content: &[u8]) -> Value {
    let (status, body) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), content).await;
    assert_eq!(
        status,
        200,
        "case {key:?}: the document must be stored, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    json!({ "bucketName": BUCKET, "key": key })
}

/// The request body for one parse: the staged `inputFile`, the requested `fileFormat` and
/// the X12 details.
fn request(input_file: Value, file_format: &str, transaction_set: &str, version: &str) -> Value {
    json!({
        "inputFile": input_file,
        "fileFormat": file_format,
        "ediType": { "x12Details": { "transactionSet": transaction_set, "version": version } }
    })
}

/// The message of a refusal, whichever casing the error shape spells it with (the model's
/// own `ValidationException` member is `Message`; the unmodelled channel's is `message`).
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// A delimiter set: the element separator, the component separator and the segment
/// terminator, which an X12 interchange declares in its own control header (the element
/// separator follows `ISA`, the component separator is ISA16 and the terminator follows
/// it).
///
/// The X12 fixture builder below ([`segment`], [`interchange`] and the transaction sets
/// they assemble) is `pub(crate)` so the `test_conversion` suite drives the X12 WRITER
/// over the very interchanges this suite drives the reader over.
pub(crate) type Delimiters = (char, char, char);

/// The delimiter set almost every X12 sender uses.
pub(crate) const STANDARD: Delimiters = ('*', '>', '~');

/// One X12 segment with `fields` as its segment id and elements, rendered with
/// `delimiters`.
pub(crate) fn segment(delimiters: Delimiters, fields: &[&str]) -> String {
    let (element, _, terminator) = delimiters;
    let separator = element.to_string();
    format!("{}{terminator}", fields.join(&separator))
}

/// One transaction set of the table below: the transaction set code, its control number,
/// and the body segments between ST and SE.
pub(crate) struct TransactionSet<'a> {
    pub(crate) code: &'a str,
    pub(crate) control: &'a str,
    pub(crate) body: Vec<Vec<&'a str>>,
}

/// The purchase-order transaction set an 850 carries: BEG with the purchase order number
/// in BEG03, a name segment and a line item.
pub(crate) fn purchase_order<'a>(control: &'a str, order: &'a str) -> TransactionSet<'a> {
    TransactionSet {
        code: "850",
        control,
        body: vec![
            vec!["BEG", "00", "SA", order, "", "20240101"],
            vec!["N1", "ST", "WAREHOUSE", "92", "0001"],
            vec!["PO1", "1", "10", "EA", "9.99"],
            vec!["CTT", "1"],
        ],
    }
}

/// A health-care-claim transaction set, whose HI segments carry COMPOSITE elements —
/// `BK>8901` is one element with two components.
pub(crate) fn health_care_claim<'a>(control: &'a str) -> TransactionSet<'a> {
    TransactionSet {
        code: "837",
        control,
        body: vec![
            vec!["BHT", "0019", "00", control, "20240101", "1200", "CH"],
            vec!["HI", "BK>8901", "BF>87200"],
        ],
    }
}

/// One functional group of a table row: its functional identifier code and the
/// transaction sets it holds.
type GroupRow<'a> = (&'a str, Vec<TransactionSet<'a>>);

/// A whole X12 interchange: ISA, then one functional group per entry in `groups`, then
/// IEA, rendered with `delimiters`.
pub(crate) fn interchange(
    delimiters: Delimiters,
    sender: &str,
    receiver: &str,
    version: &str,
    groups: &[(&str, Vec<TransactionSet<'_>>)],
) -> String {
    let (_, component, _) = delimiters;
    let component = component.to_string();
    let mut document = segment(
        delimiters,
        &[
            "ISA",
            "00",
            "          ",
            "00",
            "          ",
            "ZZ",
            sender,
            "ZZ",
            receiver,
            "240101",
            "1200",
            "U",
            version,
            "000000001",
            "0",
            "P",
            &component,
        ],
    );
    for (index, (functional_id, transactions)) in groups.iter().enumerate() {
        let control = format!("{}", index + 1);
        document.push_str(&segment(
            delimiters,
            &[
                "GS",
                functional_id,
                sender,
                receiver,
                "20240101",
                "1200",
                &control,
                "X",
                "004010",
            ],
        ));
        for transaction in transactions {
            document.push_str(&segment(
                delimiters,
                &["ST", transaction.code, transaction.control],
            ));
            for body in &transaction.body {
                document.push_str(&segment(delimiters, body));
            }
            let segments = format!("{}", transaction.body.len() + 2);
            document.push_str(&segment(
                delimiters,
                &["SE", &segments, transaction.control],
            ));
        }
        let count = format!("{}", transactions.len());
        document.push_str(&segment(delimiters, &["GE", &count, &control]));
    }
    let groups_count = format!("{}", groups.len());
    document.push_str(&segment(delimiters, &["IEA", &groups_count, "000000001"]));
    document
}

/// The `parsedFileContent` of a successful parse, as a JSON document.
fn parsed_json(case: &str, status: u16, body: &Value) -> Value {
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body:?}"
    );
    let content = body["parsedFileContent"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a parsedFileContent, got {body:?}"));
    serde_json::from_str(content).unwrap_or_else(|error| {
        panic!("case {case:?}: a JSON parse must be a JSON document, got {error}: {content}")
    })
}

/// Every element of `xml` in document order as `(name, text)`, the text empty for an
/// element with children. Panics when `xml` is not well-formed, which is how an XML parse
/// is asserted to be a document.
fn xml_elements(case: &str, xml: &str) -> Vec<(String, String)> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    let mut elements: Vec<(String, String)> = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(start)) => {
                let name = String::from_utf8_lossy(start.name().into_inner()).to_string();
                elements.push((name, String::new()));
                open.push(elements.len() - 1);
            }
            Ok(Event::Empty(start)) => {
                let name = String::from_utf8_lossy(start.name().into_inner()).to_string();
                elements.push((name, String::new()));
            }
            Ok(Event::Text(text)) => {
                if let Some(&index) = open.last() {
                    let decoded = String::from_utf8_lossy(text.into_inner().as_ref()).to_string();
                    elements[index].1.push_str(decoded.trim());
                }
            }
            Ok(Event::End(_)) => {
                open.pop();
            }
            Ok(_) => {}
            Err(error) => {
                panic!("case {case:?}: an XML parse must be well-formed XML, got {error}: {xml}")
            }
        }
    }
    elements
}

/// The body segments of a transaction, in document order, with every detail LOOP entry
/// flattened back into the segments it holds.
///
/// The canonical document groups a transaction set's repeating detail segment into ONE
/// `<SEGMENT>-8000_loop` entry holding one object per occurrence, each occurrence carrying
/// the segments inside it in its own `segments` list. Flattening is what lets a row below
/// state the segments it expects in document order whatever the grouping; the grouping
/// itself is asserted by `a_detail_loop_is_grouped_as_one_entry_per_occurrence` in
/// `canonical_document.rs`.
pub(crate) fn body_segments_in_order(segments: &[Value]) -> Vec<Value> {
    let mut flat: Vec<Value> = Vec::new();
    for entry in segments {
        let occurrences = entry.as_object().filter(|members| members.len() == 1).and_then(
            |members| {
                let (key, value) = members.iter().next()?;
                key.ends_with("_loop").then(|| value.as_array())?
            },
        );
        match occurrences {
            Some(items) => {
                for occurrence in items {
                    flat.push(occurrence.clone());
                    if let Some(inside) = occurrence.get("segments").and_then(Value::as_array) {
                        flat.extend(body_segments_in_order(inside));
                    }
                }
            }
            None => flat.push(entry.clone()),
        }
    }
    flat
}

#[tokio::test]
async fn a_parsed_document_carries_every_envelope_and_segment_the_interchange_holds() {
    let router = parsing_router();
    create_bucket(&router).await;

    // Each row: the case, the delimiter set, the sender, the receiver, the ISA12
    // interchange control version, the functional groups (with their transaction sets),
    // and the `X12TransactionSet` / `X12Version` the request names. The rows differ along
    // the delimiters, the number of groups and transaction sets, the transaction set
    // itself and whether its segments carry composite elements.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(
        &str,
        Delimiters,
        &str,
        &str,
        &str,
        Vec<(&str, Vec<TransactionSet<'_>>)>,
        &str,
        &str,
    )> = vec![
        (
            "850 purchase order, standard delimiters",
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            "00401",
            vec![("PO", vec![purchase_order("0001", "PO-12345")])],
            "X12_850",
            "VERSION_4010",
        ),
        (
            "850 purchase order, non-standard delimiters",
            ('|', ':', '\n'),
            "ACME",
            "GLOBEX",
            "00401",
            vec![("PO", vec![purchase_order("0007", "ORDER/987")])],
            "X12_850",
            "VERSION_4010",
        ),
        (
            "two transaction sets in one functional group",
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            "00501",
            vec![(
                "PO",
                vec![
                    purchase_order("0001", "PO-1"),
                    purchase_order("0002", "PO-2"),
                ],
            )],
            "X12_850",
            "VERSION_5010",
        ),
        (
            "two functional groups",
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            "00501",
            vec![
                ("PO", vec![purchase_order("0001", "PO-A")]),
                ("HC", vec![health_care_claim("0002")]),
            ],
            "X12_850",
            "VERSION_5010",
        ),
        (
            "837 health care claim with composite elements",
            STANDARD,
            "PAYER",
            "PROVIDER",
            "00501",
            vec![("HC", vec![health_care_claim("0100")])],
            "X12_837_X222",
            "VERSION_5010_HIPAA",
        ),
    ];

    for (case, delimiters, sender, receiver, isa12, groups, transaction_set, version) in cases {
        let document = interchange(delimiters, sender, receiver, isa12, &groups);
        let key = format!("in/{}.edi", case.replace([' ', ',', '/'], "-"));
        let input_file = stage(&router, &key, document.as_bytes()).await;
        let (status, body) = call(
            &router,
            "TestParsing",
            request(input_file, "JSON", transaction_set, version),
        )
        .await;
        let parsed = parsed_json(case, status, &body);

        let interchanges = parsed["interchanges"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected interchanges, got {parsed}"));
        assert_eq!(
            interchanges.len(),
            1,
            "case {case:?}: expected one interchange, got {}",
            interchanges.len()
        );
        let first = &interchanges[0];
        assert_eq!(
            first["ISA_06_SenderId"].as_str(),
            Some(sender),
            "case {case:?}: ISA06 must be the interchange's sender, got {first}"
        );
        assert_eq!(
            first["ISA_08_ReceiverId"].as_str(),
            Some(receiver),
            "case {case:?}: ISA08 must be the interchange's receiver, got {first}"
        );
        assert_eq!(
            first["IEA_01_NumberOfIncludedFunctionalGroups"].as_str(),
            Some(groups.len().to_string().as_str()),
            "case {case:?}: IEA01 must be the number of functional groups, got {first}"
        );
        let parsed_groups = first["functional_groups"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected functional_groups, got {first}"));
        assert_eq!(
            parsed_groups.len(),
            groups.len(),
            "case {case:?}: expected {} functional groups, got {}",
            groups.len(),
            parsed_groups.len()
        );

        for (index, (functional_id, transactions)) in groups.iter().enumerate() {
            let group = &parsed_groups[index];
            assert_eq!(
                group["GS_01_FunctionalIdentifierCode"].as_str(),
                Some(*functional_id),
                "case {case:?}: GS01 must be the group's functional identifier, got {group}"
            );
            assert_eq!(
                group["GE_01_NumberOfTransactionSetsIncluded"].as_str(),
                Some(transactions.len().to_string().as_str()),
                "case {case:?}: GE01 must be the number of transaction sets, got {group}"
            );
            let parsed_transactions = group["transactions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected transactions, got {group}"));
            assert_eq!(
                parsed_transactions.len(),
                transactions.len(),
                "case {case:?}: expected {} transaction sets, got {}",
                transactions.len(),
                parsed_transactions.len()
            );

            for (position, transaction) in transactions.iter().enumerate() {
                let parsed_transaction = &parsed_transactions[position];
                assert_eq!(
                    parsed_transaction["ST_01_TransactionSetIdentifierCode"].as_str(),
                    Some(transaction.code),
                    "case {case:?}: ST01 must be the transaction set code, got {parsed_transaction}"
                );
                assert_eq!(
                    parsed_transaction["ST_02_TransactionSetControlNumber"].as_str(),
                    Some(transaction.control),
                    "case {case:?}: ST02 must be the transaction set control number, got \
                     {parsed_transaction}"
                );
                assert_eq!(
                    parsed_transaction["SE_02_TransactionSetControlNumber"].as_str(),
                    Some(transaction.control),
                    "case {case:?}: SE02 must close the transaction set it opened, got \
                     {parsed_transaction}"
                );
                let body_segments = body_segments_in_order(
                    parsed_transaction["segments"].as_array().unwrap_or_else(|| {
                        panic!("case {case:?}: expected the transaction's segments, got {parsed_transaction}")
                    }),
                );
                assert_eq!(
                    body_segments.len(),
                    transaction.body.len(),
                    "case {case:?}: expected {} body segments, got {}",
                    transaction.body.len(),
                    body_segments.len()
                );
                for (index, expected) in transaction.body.iter().enumerate() {
                    let parsed_segment = &body_segments[index];
                    assert_eq!(
                        parsed_segment["segment_id"].as_str(),
                        Some(expected[0]),
                        "case {case:?}: segment {index} must be {:?}, got {parsed_segment}",
                        expected[0]
                    );
                    for (element, value) in expected.iter().enumerate().skip(1) {
                        let key = format!("{}_{element:02}", expected[0]);
                        if value.is_empty() {
                            // An element present but EMPTY is the X12 encoding of an
                            // absent element, so the parse carries no value for it.
                            assert!(
                                parsed_segment.get(&key).is_none(),
                                "case {case:?}: {key} is empty in the document, so it must \
                                 be absent from the parse, got {parsed_segment}"
                            );
                        } else if value.contains('>') {
                            // A COMPOSITE element: its components are keyed by segment,
                            // position and component.
                            let components: Vec<&str> = value.split('>').collect();
                            for (component, text) in components.iter().enumerate() {
                                let component_key = format!("{key}_{:02}", component + 1);
                                assert_eq!(
                                    parsed_segment[&key][&component_key].as_str(),
                                    Some(*text),
                                    "case {case:?}: {component_key} must be {text:?}, got \
                                     {parsed_segment}"
                                );
                            }
                        } else {
                            assert_eq!(
                                parsed_segment[&key].as_str(),
                                Some(*value),
                                "case {case:?}: {key} must be {value:?}, got {parsed_segment}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn both_document_formats_report_the_same_interchange() {
    let router = parsing_router();
    create_bucket(&router).await;

    // The same document parsed as JSON and as XML: the two renderings are the same tree,
    // so every value one carries the other carries. The rows differ along the delimiter
    // set and the transaction set, and ISA11's NAME follows the X12 release — the
    // Repetition Separator from release 004020, the Interchange Control Standards
    // Identifier before it (ASC X12 interchange control header).
    let cases: Vec<(&str, Delimiters, &str, &str, &str)> = vec![
        (
            "release 4010",
            STANDARD,
            "VERSION_4010",
            "00401",
            "ISA_11_InterchangeControlStandardsId",
        ),
        (
            "release 5010",
            ('*', ':', '~'),
            "VERSION_5010",
            "00501",
            "ISA_11_RepetitionSeparator",
        ),
        (
            "release 4030",
            ('+', '>', '\''),
            "VERSION_4030",
            "00403",
            "ISA_11_RepetitionSeparator",
        ),
    ];

    for (case, delimiters, version, isa12, isa11) in cases {
        let document = interchange(
            delimiters,
            "SENDERID",
            "RECEIVERID",
            isa12,
            &[("PO", vec![purchase_order("0001", "PO-77")])],
        );
        let key = format!("formats/{case}.edi").replace(' ', "-");
        let input_file = stage(&router, &key, document.as_bytes()).await;

        let (status, body) = call(
            &router,
            "TestParsing",
            request(input_file.clone(), "JSON", "X12_850", version),
        )
        .await;
        let parsed = parsed_json(case, status, &body);
        let transaction = &parsed["interchanges"][0]["functional_groups"][0]["transactions"][0];
        assert_eq!(
            transaction["segments"][0]["BEG_03"].as_str(),
            Some("PO-77"),
            "case {case:?}: the JSON parse must carry BEG03, got {transaction}"
        );
        assert!(
            parsed["interchanges"][0].get(isa11).is_some(),
            "case {case:?}: ISA11 must be <{isa11}> at this release, got {}",
            parsed["interchanges"][0]
        );

        let (status, body) = call(
            &router,
            "TestParsing",
            request(input_file, "XML", "X12_850", version),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an XML parse, got {status} with {body:?}"
        );
        let xml = body["parsedFileContent"].as_str().unwrap_or_default();
        let elements = xml_elements(case, xml);
        assert_eq!(
            elements.first().map(|(name, _)| name.as_str()),
            Some("interchanges"),
            "case {case:?}: the XML parse's root element must be <interchanges>, got {:?}",
            elements.first()
        );
        for (name, expected) in [
            ("ISA_06_SenderId", "SENDERID"),
            ("GS_01_FunctionalIdentifierCode", "PO"),
            ("ST_01_TransactionSetIdentifierCode", "850"),
            ("BEG_03", "PO-77"),
        ] {
            let found: Vec<&(String, String)> =
                elements.iter().filter(|(key, _)| key == name).collect();
            assert!(
                found.len() == 1 && found[0].1 == expected,
                "case {case:?}: the XML parse must carry <{name}>{expected}</{name}>, got {found:?}"
            );
        }
        assert!(
            elements.iter().any(|(name, _)| name == isa11),
            "case {case:?}: the XML parse must carry <{isa11}>, got {:?}",
            elements.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
    }
}

#[tokio::test]
async fn splitting_by_transaction_answers_one_parsed_file_per_transaction_set() {
    let router = parsing_router();
    create_bucket(&router).await;

    // Each row: the case, the functional groups the document holds, and the transaction
    // set control numbers the split files must carry, in order. The rows differ along the
    // number of transaction sets and the number of groups they are spread over.
    let cases: Vec<(&str, Vec<GroupRow<'_>>, Vec<&str>)> = vec![
        (
            "one transaction set",
            vec![("PO", vec![purchase_order("0001", "PO-1")])],
            vec!["0001"],
        ),
        (
            "three transaction sets in one group",
            vec![(
                "PO",
                vec![
                    purchase_order("0001", "PO-1"),
                    purchase_order("0002", "PO-2"),
                    purchase_order("0003", "PO-3"),
                ],
            )],
            vec!["0001", "0002", "0003"],
        ),
        (
            "transaction sets spread over two groups",
            vec![
                (
                    "PO",
                    vec![
                        purchase_order("0001", "PO-1"),
                        purchase_order("0002", "PO-2"),
                    ],
                ),
                ("HC", vec![health_care_claim("0003")]),
            ],
            vec!["0001", "0002", "0003"],
        ),
    ];

    for (case, groups, expected) in cases {
        let document = interchange(STANDARD, "SENDERID", "RECEIVERID", "00401", &groups);
        let key = format!("split/{case}.edi").replace(' ', "-");
        let input_file = stage(&router, &key, document.as_bytes()).await;

        let mut body = request(input_file.clone(), "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] = json!({ "x12": { "splitOptions": { "splitBy": "TRANSACTION" } } });
        let (status, response) = call(&router, "TestParsing", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {response:?}"
        );
        let contents = response["parsedSplitFileContents"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected parsedSplitFileContents, got {response:?}")
            });
        assert_eq!(
            contents.len(),
            expected.len(),
            "case {case:?}: expected {} split files, got {}",
            expected.len(),
            contents.len()
        );

        for (index, control) in expected.iter().enumerate() {
            let text = contents[index].as_str().unwrap_or_default();
            let split: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: each split file must be a JSON document, got {error}: {text}")
            });
            let split_groups = split["interchanges"][0]["functional_groups"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected one group per split file, got {split}")
                });
            assert_eq!(
                split_groups.len(),
                1,
                "case {case:?}: split file {index} must hold one functional group, got {}",
                split_groups.len()
            );
            let transactions = split_groups[0]["transactions"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected transactions, got {split}"));
            assert_eq!(
                transactions.len(),
                1,
                "case {case:?}: split file {index} must hold one transaction set, got {}",
                transactions.len()
            );
            assert_eq!(
                transactions[0]["ST_02_TransactionSetControlNumber"].as_str(),
                Some(*control),
                "case {case:?}: split file {index} must hold transaction set {control:?}, got \
                 {split}"
            );
            // A split file holds ONE transaction set in ONE group, so the control counts
            // its envelope reports are the ones it really holds.
            assert_eq!(
                split_groups[0]["GE_01_NumberOfTransactionSetsIncluded"].as_str(),
                Some("1"),
                "case {case:?}: split file {index} must report GE01 as 1, got {split}"
            );
            assert_eq!(
                split["interchanges"][0]["IEA_01_NumberOfIncludedFunctionalGroups"].as_str(),
                Some("1"),
                "case {case:?}: split file {index} must report IEA01 as 1, got {split}"
            );
        }

        // The whole document is still what `parsedFileContent` reports, whatever the
        // split options say.
        let parsed: Value =
            serde_json::from_str(response["parsedFileContent"].as_str().unwrap_or_default())
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: parsedFileContent must be a JSON document: {error}")
                });
        let total: usize = parsed["interchanges"][0]["functional_groups"]
            .as_array()
            .map(|groups| {
                groups
                    .iter()
                    .map(|group| {
                        group["transactions"]
                            .as_array()
                            .map(Vec::len)
                            .unwrap_or_default()
                    })
                    .sum()
            })
            .unwrap_or_default();
        assert_eq!(
            total,
            expected.len(),
            "case {case:?}: parsedFileContent must carry every transaction set, got {total}"
        );

        // Splitting is what the request asks for: with NONE, and with no split options at
        // all, there are no split files.
        for (split_case, advanced) in [
            (
                "splitBy NONE",
                Some(json!({ "x12": { "splitOptions": { "splitBy": "NONE" } } })),
            ),
            ("no advancedOptions", None),
        ] {
            let mut body = request(input_file.clone(), "JSON", "X12_850", "VERSION_4010");
            if let Some(advanced) = advanced {
                body["advancedOptions"] = advanced;
            }
            let (status, response) = call(&router, "TestParsing", body).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {split_case:?}: expected HTTP 200, got {status} with \
                 body {response:?}"
            );
            assert!(
                response.get("parsedSplitFileContents").is_none(),
                "case {case:?} / {split_case:?}: nothing was split, so no split contents \
                 must be reported, got {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn content_that_is_not_an_x12_interchange_is_refused() {
    let router = parsing_router();
    create_bucket(&router).await;

    let valid = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-1")])],
    );

    // Each row: the case, the object's bytes, and a fragment the refusal must name
    // (`None` where only the classification is asserted). The rows differ along the way
    // the content fails to be an interchange: not EDI at all, empty, an ISA that does not
    // declare delimiters, a truncated header, and envelopes that do not nest.
    let cases: Vec<(&str, Vec<u8>, Option<&str>)> = vec![
        (
            "plain text",
            b"this is plainly not an X12 EDI document".to_vec(),
            Some("No supported EDI standard interchange begins with thi"),
        ),
        (
            "empty object",
            Vec::new(),
            Some("No supported EDI standard interchange begins with"),
        ),
        (
            "an EDIFACT interchange, which this emulator does not parse",
            b"UNB+UNOA:1+SENDER+RECEIVER+240101:1200+1'UNZ+1+1'".to_vec(),
            Some("No supported EDI standard interchange begins with UNB"),
        ),
        (
            "ISA followed by a data character instead of a delimiter",
            b"ISANOTANINTERCHANGE".to_vec(),
            Some("No supported EDI standard interchange begins with ISA"),
        ),
        (
            "a header truncated before its sixteen elements",
            b"ISA*00*          *00*          *ZZ*SENDERID".to_vec(),
            Some("interchange control header"),
        ),
        (
            "an interchange with no trailer",
            valid.replace("IEA*1*000000001~", "").into_bytes(),
            Some("not closed by its trailer (IEA)"),
        ),
        (
            // The interchange trailer then closes an interchange whose group is open.
            "a functional group with no trailer",
            valid.replace("GE*1*1~", "").into_bytes(),
            Some("is still open"),
        ),
        (
            // The group trailer then closes a group whose transaction set is open.
            "a transaction set with no trailer",
            valid.replace("SE*6*0001~", "").into_bytes(),
            Some("is still open"),
        ),
        (
            "an interchange whose trailer comes before its transaction set closes",
            valid
                .replace("SE*6*0001~GE*1*1~", "")
                .into_bytes(),
            Some("is still open"),
        ),
        (
            "a transaction set header outside any functional group",
            valid.replace("GS*PO*SENDERID*RECEIVERID*20240101*1200*1*X*004010~", "")
                .into_bytes(),
            Some("outside any functional group"),
        ),
        (
            // Truncated mid-transaction: nothing closes any of the three envelopes.
            "a document truncated after its transaction set header",
            valid
                .split("SE*6*0001~")
                .next()
                .unwrap_or_default()
                .as_bytes()
                .to_vec(),
            Some("not closed by its trailer (SE)"),
        ),
        (
            // An X12 segment id begins with a letter, and a key built from one must be a
            // legal XML element name for the `XML` rendering.
            "a segment whose identifier does not begin with a letter",
            valid.replace("N1*ST*", "1N*ST*").into_bytes(),
            Some("is not an X12 segment identifier"),
        ),
        (
            "bytes that are not UTF-8 text",
            {
                let mut bytes = valid.clone().into_bytes();
                bytes.splice(60..60, [0xff, 0xfe]);
                bytes
            },
            Some("not UTF-8 text"),
        ),
    ];

    for (case, content, fragment) in cases {
        let key = format!("bad/{}.edi", case.replace([' ', ',', '/'], "-"));
        let input_file = stage(&router, &key, &content).await;
        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file, "JSON", "X12_850", "VERSION_4010"),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
        );
        if let Some(fragment) = fragment {
            let message = message(&response);
            assert!(
                message.contains(fragment),
                "case {case:?}: the refusal must name {fragment:?}, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_file_larger_than_the_documented_limit_is_refused() {
    let router = parsing_router();
    create_bucket(&router).await;

    // AWS documents the limit on this operation: "The input file has a file size limit of
    // 250 KB". The rows are one interchange just inside the limit and one just past it,
    // padded with repeated body segments.
    let limit = service_com_amazonaws_b2bi_b2bi::ops::test_parsing::MAX_INPUT_FILE_BYTES;
    // One interchange whose transaction set carries `padding` repeated `REF*ZZ*PADDING~`
    // segments, so the document's size is chosen rather than guessed.
    let build = |padding: usize| {
        let mut body: Vec<Vec<&str>> = (0..padding).map(|_| vec!["REF", "ZZ", "PADDING"]).collect();
        body.push(vec!["CTT", "1"]);
        interchange(
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            "00401",
            &[(
                "PO",
                vec![TransactionSet {
                    code: "850",
                    control: "0001",
                    body,
                }],
            )],
        )
    };
    let overhead = build(0).len();
    let per_segment = build(1).len() - overhead;

    for (case, over_limit) in [("inside the limit", false), ("past the limit", true)] {
        let room = limit.saturating_sub(overhead) / per_segment;
        let document = build(if over_limit { room + 2 } else { room - 1 });
        assert_eq!(
            document.len() > limit,
            over_limit,
            "case {case:?}: the document must be {} the limit, got {} bytes",
            if over_limit { "past" } else { "inside" },
            document.len()
        );

        let key = format!("size/{case}.edi").replace(' ', "-");
        let input_file = stage(&router, &key, document.as_bytes()).await;
        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file, "JSON", "X12_850", "VERSION_4010"),
        )
        .await;
        if over_limit {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response:?}"
            );
            assert!(
                message(&response).contains(&limit.to_string()),
                "case {case:?}: the refusal must name the documented limit, got {:?}",
                message(&response)
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a document inside the limit must parse, got {status} with \
                 body {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_input_file_that_names_no_stored_object_is_refused_as_not_found() {
    let router = parsing_router();
    create_bucket(&router).await;
    let staged = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-1")])],
    );
    stage(&router, "in/present.edi", staged.as_bytes()).await;

    // Each row: the case, the `inputFile`, and the expected status. A location that names
    // no object is not-found; a location that names no object AT ALL — an absent
    // bucket name or key — is a bad request (`ValidationException`), which a client fixes
    // differently.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "a key the bucket does not hold",
            json!({ "bucketName": BUCKET, "key": "in/absent.edi" }),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        ),
        (
            "a bucket this emulator does not hold",
            json!({ "bucketName": "no-such-bucket-here", "key": "in/present.edi" }),
            NOT_FOUND_STATUS,
            "ResourceNotFoundException",
        ),
        (
            "no key",
            json!({ "bucketName": BUCKET }),
            400,
            "ValidationException",
        ),
        (
            "no bucket name",
            json!({ "key": "in/present.edi" }),
            400,
            "ValidationException",
        ),
        ("an empty location", json!({}), 400, "ValidationException"),
    ];

    for (case, input_file, expected_status, expected_code) in cases {
        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file, "JSON", "X12_850", "VERSION_4010"),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {response:?}"
        );
        assert_eq!(
            error_code(&response),
            expected_code,
            "case {case:?}: expected a {expected_code}, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_file_format_that_names_no_document_format_is_refused() {
    let router = parsing_router();
    create_bucket(&router).await;
    let document = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-1")])],
    );
    let input_file = stage(&router, "format/document.edi", document.as_bytes()).await;

    // The model's own `FileFormat` member list, so a value the model gains cannot silently
    // stay unhandled: `JSON` and `XML` are the formats a parse is answered in, and
    // `NOT_USED` names none of them.
    for file_format in service_com_amazonaws_b2bi_b2bi::ops::types::FILE_FORMAT_VALUES {
        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file.clone(), file_format, "X12_850", "VERSION_4010"),
        )
        .await;
        if *file_format == "NOT_USED" {
            assert_eq!(
                status, 400,
                "case {file_format:?}: expected HTTP 400, got {status} with body {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {file_format:?}: expected a ValidationException, got {response:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {file_format:?}: expected HTTP 200, got {status} with body {response:?}"
            );
            assert!(
                response["parsedFileContent"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("PO-1"),
                "case {file_format:?}: the parse must carry the document's values, got \
                 {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn configured_element_requirement_rules_report_what_the_document_does_not_carry() {
    let router = parsing_router();
    create_bucket(&router).await;
    // BEG04 (the release number) is EMPTY in this document and BEG05 carries a date, so a
    // rule over each one has an answer the document decides.
    let document = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[(
            "PO",
            vec![
                purchase_order("0001", "PO-1"),
                purchase_order("0002", "PO-2"),
            ],
        )],
    );
    let input_file = stage(&router, "rules/document.edi", document.as_bytes()).await;

    // Each row: the case, the configured rules, and how many validation messages the
    // document must produce. The rows differ along the element position, the requirement
    // and the number of occurrences of the segment the rule names.
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "a mandatory element the document carries",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-03", "requirement": "MANDATORY" } }]),
            0,
        ),
        (
            "a mandatory element the document leaves empty, in both transaction sets",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "MANDATORY" } }]),
            2,
        ),
        (
            "a mandatory envelope element the header does not carry",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "ST-03", "requirement": "MANDATORY" } }]),
            2,
        ),
        (
            "the same element made optional",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "OPTIONAL" } }]),
            0,
        ),
        (
            "a rule naming a segment the document does not hold",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BAK-01", "requirement": "MANDATORY" } }]),
            0,
        ),
        (
            "two rules at once",
            json!([
                { "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "MANDATORY" } },
                { "elementRequirementValidationRule": { "elementPosition": "N1-05", "requirement": "MANDATORY" } }
            ]),
            4,
        ),
    ];

    for (case, rules, expected) in cases {
        let mut body = request(input_file.clone(), "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] =
            json!({ "x12": { "validationOptions": { "validationRules": rules } } });
        let (status, response) = call(&router, "TestParsing", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {response:?}"
        );
        let messages = response["validationMessages"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            messages.len(),
            expected,
            "case {case:?}: expected {expected} validation messages, got {messages:?}"
        );
    }

    // A rule keyed by a four-digit X12 DATA ELEMENT id cannot be applied without the X12
    // data element dictionary, so it is refused rather than reported as a validation that
    // passed.
    for (case, rules) in [
        (
            "an element length rule",
            json!([{ "elementLengthValidationRule": { "elementId": "0127", "minLength": 1, "maxLength": 30 } }]),
        ),
        (
            "a code list rule",
            json!([{ "codeListValidationRule": { "elementId": "0092", "codesToAdd": ["ZZ"] } }]),
        ),
    ] {
        let mut body = request(input_file.clone(), "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] =
            json!({ "x12": { "validationOptions": { "validationRules": rules } } });
        let (status, response) = call(&router, "TestParsing", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_split_file_carries_its_own_envelope_and_not_the_files_loose_segments() {
    let router = parsing_router();
    create_bucket(&router).await;

    // A segment inside the interchange or the group but outside every transaction set
    // (TA1 here for the interchange, LS for the group) belongs to the input file as a
    // whole: the whole-file parse carries it, and a per-transaction split file does not.
    let document = "ISA*00*          *00*          *ZZ*SENDERID       *ZZ*RECEIVERID    *\
         240101*1200*U*00401*000000001*0*P*>~TA1*000000001*240101*1200*A*000~\
         GS*PO*SENDERID*RECEIVERID*20240101*1200*1*X*004010~LS*0100~\
         ST*850*0001~BEG*00*SA*PO-1**20240101~SE*3*0001~\
         ST*850*0002~BEG*00*SA*PO-2**20240101~SE*3*0002~GE*2*1~IEA*1*000000001~";
    let input_file = stage(&router, "loose/document.edi", document.as_bytes()).await;

    let mut body = request(input_file, "JSON", "X12_850", "VERSION_4010");
    body["advancedOptions"] = json!({ "x12": { "splitOptions": { "splitBy": "TRANSACTION" } } });
    let (status, response) = call(&router, "TestParsing", body).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200, got {status} with body {response:?}"
    );

    let whole: Value =
        serde_json::from_str(response["parsedFileContent"].as_str().unwrap_or_default())
            .expect("parsedFileContent must be a JSON document");
    let interchange = &whole["interchanges"][0];
    assert_eq!(
        interchange["segments"][0]["segment_id"].as_str(),
        Some("TA1"),
        "the whole-file parse must carry the interchange's loose segment, got {interchange}"
    );
    assert_eq!(
        interchange["functional_groups"][0]["segments"][0]["segment_id"].as_str(),
        Some("LS"),
        "the whole-file parse must carry the group's loose segment, got {interchange}"
    );

    let contents = response["parsedSplitFileContents"]
        .as_array()
        .unwrap_or_else(|| panic!("expected parsedSplitFileContents, got {response:?}"));
    assert_eq!(
        contents.len(),
        2,
        "expected one split file per transaction set, got {}",
        contents.len()
    );
    for (index, content) in contents.iter().enumerate() {
        let split: Value = serde_json::from_str(content.as_str().unwrap_or_default())
            .unwrap_or_else(|error| panic!("split file {index} must be JSON: {error}"));
        let split_interchange = &split["interchanges"][0];
        assert!(
            split_interchange.get("segments").is_none(),
            "split file {index} must not carry the interchange's loose segments, got \
             {split_interchange}"
        );
        assert!(
            split_interchange["functional_groups"][0]
                .get("segments")
                .is_none(),
            "split file {index} must not carry the group's loose segments, got \
             {split_interchange}"
        );
        assert_eq!(
            split_interchange["ISA_06_SenderId"].as_str(),
            // The ISA's elements are fixed width in this document, and a parse keeps an
            // element's value verbatim — padding included.
            Some("SENDERID       "),
            "split file {index} must carry the interchange envelope it came from, got {split}"
        );
    }
}

#[tokio::test]
async fn a_split_or_a_rule_list_past_this_emulators_bounds_is_refused() {
    use service_com_amazonaws_b2bi_b2bi::ops::test_parsing::{
        MAX_SPLIT_FILES, MAX_VALIDATION_RULES,
    };

    let router = parsing_router();
    create_bucket(&router).await;

    // The number of transaction sets is a factor of the split output that the 250 KB
    // input limit does not bound on its own (a minimal ST/SE pair is a couple of dozen
    // bytes), so it carries its own bound: at the bound a split is answered, past it the
    // request is refused.
    for (case, transaction_sets) in [
        ("at the split-file bound", MAX_SPLIT_FILES),
        ("past the split-file bound", MAX_SPLIT_FILES + 1),
    ] {
        let controls: Vec<String> = (0..transaction_sets)
            .map(|index| format!("{:04}", index + 1))
            .collect();
        let transactions: Vec<TransactionSet<'_>> = controls
            .iter()
            .map(|control| TransactionSet {
                code: "850",
                control,
                body: Vec::new(),
            })
            .collect();
        let document = interchange(
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            "00401",
            &[("PO", transactions)],
        );
        let key = format!("bounds/{case}.edi").replace(' ', "-");
        let input_file = stage(&router, &key, document.as_bytes()).await;
        let mut body = request(input_file, "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] = json!({ "x12": { "splitOptions": { "splitBy": "TRANSACTION" } } });
        let (status, response) = call(&router, "TestParsing", body).await;

        if transaction_sets > MAX_SPLIT_FILES {
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response:?}"
            );
            assert!(
                message(&response).contains(&MAX_SPLIT_FILES.to_string()),
                "case {case:?}: the refusal must name the bound, got {:?}",
                message(&response)
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a split at the bound must be answered, got {status}"
            );
            assert_eq!(
                response["parsedSplitFileContents"]
                    .as_array()
                    .map(Vec::len)
                    .unwrap_or_default(),
                transaction_sets,
                "case {case:?}: expected {transaction_sets} split files"
            );
        }
    }

    // The configured rule list is the other factor the request chooses.
    let document = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-1")])],
    );
    let input_file = stage(&router, "bounds/rules.edi", document.as_bytes()).await;
    for (case, count) in [
        ("at the rule bound", MAX_VALIDATION_RULES),
        ("past the rule bound", MAX_VALIDATION_RULES + 1),
    ] {
        let rules: Vec<Value> = (0..count)
            .map(|_| {
                json!({ "elementRequirementValidationRule": { "elementPosition": "BEG-03", "requirement": "MANDATORY" } })
            })
            .collect();
        let mut body = request(input_file.clone(), "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] =
            json!({ "x12": { "validationOptions": { "validationRules": rules } } });
        let (status, response) = call(&router, "TestParsing", body).await;
        let expected = if count > MAX_VALIDATION_RULES { 400 } else { 200 };
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with body {response:?}"
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response:?}"
            );
        }
    }
}

#[tokio::test]
async fn element_values_carrying_markup_characters_survive_both_renderings() {
    let router = parsing_router();
    create_bucket(&router).await;

    // An element's value is business data, so it may carry the characters JSON and XML
    // reserve. Both renderings must escape them and both must read back VERBATIM. The
    // delimiter set makes `>` an ordinary character here (the component separator is
    // `:`), so a value holding it is one element and not a composite.
    let delimiters: Delimiters = ('*', ':', '~');
    let cases: Vec<(&str, &str)> = vec![
        ("xml metacharacters", "A<B&C>D"),
        ("quotes", "SAY \"HELLO\" & 'BYE'"),
        ("a document-looking value", "<?xml version=\"1.0\"?>"),
        ("json metacharacters", "A\\B/C\"D"),
        ("non-ascii text", "ÉTÉ-2024 \u{20ac}99"),
    ];

    for (case, value) in cases {
        let document = interchange(
            delimiters,
            "SENDERID",
            "RECEIVERID",
            "00401",
            &[(
                "PO",
                vec![TransactionSet {
                    code: "850",
                    control: "0001",
                    body: vec![vec!["BEG", "00", "SA", value, "", "20240101"]],
                }],
            )],
        );
        let key = format!("markup/{}.edi", case.replace(' ', "-"));
        let input_file = stage(&router, &key, document.as_bytes()).await;

        let (status, body) = call(
            &router,
            "TestParsing",
            request(input_file.clone(), "JSON", "X12_850", "VERSION_4010"),
        )
        .await;
        let parsed = parsed_json(case, status, &body);
        assert_eq!(
            parsed["interchanges"][0]["functional_groups"][0]["transactions"][0]["segments"][0]
                ["BEG_03"]
                .as_str(),
            Some(value),
            "case {case:?}: the JSON parse must carry BEG03 verbatim, got {parsed}"
        );

        let (status, body) = call(
            &router,
            "TestParsing",
            request(input_file, "XML", "X12_850", "VERSION_4010"),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for an XML parse, got {status} with {body:?}"
        );
        let xml = body["parsedFileContent"].as_str().unwrap_or_default();
        // The document is well-formed (`xml_elements` parses it and panics otherwise)...
        let elements = xml_elements(case, xml);
        assert!(
            elements.iter().any(|(name, _)| name == "BEG_03"),
            "case {case:?}: the XML parse must carry <BEG_03>, got {:?}",
            elements.iter().map(|(name, _)| name).collect::<Vec<_>>()
        );
        // ... the value is escaped in it rather than written through raw ...
        let escaped = quick_xml::escape::escape(value);
        assert!(
            xml.contains(&format!("<BEG_03>{escaped}</BEG_03>")),
            "case {case:?}: the XML parse must carry BEG03 escaped as {escaped:?}, got {xml}"
        );
        // ... and unescaping what it carries gives the element's value back verbatim.
        assert_eq!(
            quick_xml::escape::unescape(&escaped)
                .unwrap_or_default()
                .as_ref(),
            value,
            "case {case:?}: BEG03 must decode back to the document's own value"
        );
    }
}

#[tokio::test]
async fn parsing_a_document_changes_nothing_a_later_caller_sees() {
    let router = parsing_router();
    create_bucket(&router).await;
    let document = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-1")])],
    );
    let input_file = stage(&router, "idempotent/document.edi", document.as_bytes()).await;

    // The operation owns no resource: it is a function of the stored object, so a repeat
    // cannot differ, and the object it read is still there byte for byte afterwards.
    let body = request(input_file, "JSON", "X12_850", "VERSION_4010");
    let (first_status, first) = call(&router, "TestParsing", body.clone()).await;
    let (second_status, second) = call(&router, "TestParsing", body).await;
    assert_eq!(
        (first_status, second_status),
        (200, 200),
        "a repeated parse must succeed twice, got {first_status} then {second_status}"
    );
    assert_eq!(
        first["parsedFileContent"], second["parsedFileContent"],
        "a repeated parse must answer the same document, got {first:?} then {second:?}"
    );

    let (status, stored) = s3(
        &router,
        "GET",
        &format!("/{BUCKET}/idempotent/document.edi"),
        b"",
    )
    .await;
    assert_eq!(
        status, 200,
        "the parsed object must still be readable, got {status}"
    );
    assert_eq!(
        String::from_utf8_lossy(&stored),
        document,
        "parsing must leave the stored object byte for byte"
    );
}

/// Element 11 of the interchange control header before X12 release 004020, and from it on.
///
/// Source: the element table of the ASC X12 interchange control header — position 11 is
/// the Interchange Control Standards Identifier until release 004020, where the Repetition
/// Separator takes it.
const ISA11_PRE_4020: &str = "ISA_11_InterchangeControlStandardsId";
const ISA11_FROM_4020: &str = "ISA_11_RepetitionSeparator";

#[tokio::test]
async fn isa11_follows_the_release_the_interchange_declares_when_the_request_names_none() {
    let router = parsing_router();
    create_bucket(&router).await;

    // `X12Details.version` is OPTIONAL (`smithy_query` query=shape shape=X12Details), so a
    // request may name no release at all; then the interchange's own ISA12 Interchange
    // Control Version Number is what names ISA11. `X12Version` cannot even express release
    // 004020 — its values are VERSION_4010, 4030, 4050, 4060, 5010, 5010_HIPAA — so an
    // interchange AT the release where the Repetition Separator appears is reachable only
    // this way. The rows span below the bound, the bound itself and past it.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("release 003020, below the bound", "00302", ISA11_PRE_4020),
        ("release 004010, below the bound", "00401", ISA11_PRE_4020),
        ("release 004020, the bound", "00402", ISA11_FROM_4020),
        ("release 004030, one past the bound", "00403", ISA11_FROM_4020),
        ("release 005010, past the bound", "00501", ISA11_FROM_4020),
    ];

    for (case, isa12, expected) in cases {
        let document = interchange(
            STANDARD,
            "SENDERID",
            "RECEIVERID",
            isa12,
            &[("PO", vec![purchase_order("0001", "PO-11")])],
        );
        let key = format!("declared-release/{isa12}.edi");
        let input_file = stage(&router, &key, document.as_bytes()).await;

        // The request names the transaction set and NO version, which is why the parse has
        // only the interchange itself to take the release from.
        let body = json!({
            "inputFile": input_file,
            "fileFormat": "JSON",
            "ediType": { "x12Details": { "transactionSet": "X12_850" } }
        });
        let (status, response) = call(&router, "TestParsing", body).await;
        let parsed = parsed_json(case, status, &response);
        let reported = &parsed["interchanges"][0];

        // ISA11's value is what the interchange carries at position 11 either way; it is
        // the member's NAME that follows the release.
        assert_eq!(
            reported[expected].as_str(),
            Some("U"),
            "case {case:?}: ISA11 must be reported as <{expected}> carrying the \
             interchange's own value, got {reported}"
        );
        let other = if expected == ISA11_PRE_4020 {
            ISA11_FROM_4020
        } else {
            ISA11_PRE_4020
        };
        assert!(
            reported.get(other).is_none(),
            "case {case:?}: ISA11 must be reported once, not also as <{other}>, got {reported}"
        );
        assert_eq!(
            reported["ISA_12_Version"].as_str(),
            Some(isa12),
            "case {case:?}: ISA12 must be the release the interchange declares, got {reported}"
        );
    }

    // ... and a request that DOES name a version is what the release follows, which is the
    // other half of the handler's documented rule ("the one the request names, else the one
    // the interchange declares in ISA12"): the same document, read at the release the
    // request names, reports ISA11 by that release's name.
    let document = interchange(
        STANDARD,
        "SENDERID",
        "RECEIVERID",
        "00401",
        &[("PO", vec![purchase_order("0001", "PO-11")])],
    );
    let input_file = stage(&router, "declared-release/named.edi", document.as_bytes()).await;
    for (case, version, expected) in [
        ("the request names 4010", "VERSION_4010", ISA11_PRE_4020),
        ("the request names 5010", "VERSION_5010", ISA11_FROM_4020),
    ] {
        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file.clone(), "JSON", "X12_850", version),
        )
        .await;
        let parsed = parsed_json(case, status, &response);
        let reported = &parsed["interchanges"][0];
        assert_eq!(
            reported[expected].as_str(),
            Some("U"),
            "case {case:?}: the release the request names must name ISA11 <{expected}>, got \
             {reported}"
        );
    }
}

#[tokio::test]
async fn every_interchange_a_file_holds_is_parsed_in_file_order() {
    let router = parsing_router();
    create_bucket(&router).await;

    // A parsed document's `interchanges` is a LIST, so the rule generalises over how many
    // interchanges a file holds: each one is reported, in file order, with its own
    // envelope and its own control counts. Each row is one file, with one entry per
    // interchange in it — the sender that identifies it, its ISA12 and its functional
    // groups — and the rows differ along the number of interchanges and along how many
    // groups and transaction sets each of them holds.
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Vec<(&str, &str, Vec<GroupRow<'_>>)>)> = vec![
        (
            "one interchange",
            vec![(
                "SENDERONE",
                "00401",
                vec![("PO", vec![purchase_order("0001", "PO-1")])],
            )],
        ),
        (
            "two interchanges",
            vec![
                (
                    "SENDERONE",
                    "00401",
                    vec![("PO", vec![purchase_order("0001", "PO-1")])],
                ),
                (
                    "SENDERTWO",
                    "00501",
                    vec![("HC", vec![health_care_claim("0002")])],
                ),
            ],
        ),
        (
            "three interchanges, the middle one holding two groups",
            vec![
                (
                    "SENDERONE",
                    "00401",
                    vec![("PO", vec![purchase_order("0001", "PO-1")])],
                ),
                (
                    "SENDERTWO",
                    "00401",
                    vec![
                        (
                            "PO",
                            vec![
                                purchase_order("0002", "PO-2"),
                                purchase_order("0003", "PO-3"),
                            ],
                        ),
                        ("HC", vec![health_care_claim("0004")]),
                    ],
                ),
                (
                    "SENDERTHREE",
                    "00401",
                    vec![("PO", vec![purchase_order("0005", "PO-5")])],
                ),
            ],
        ),
    ];

    for (case, file) in cases {
        let mut document = String::new();
        for (sender, isa12, groups) in &file {
            document.push_str(&interchange(STANDARD, sender, "RECEIVERID", isa12, groups));
        }
        let key = format!("multi/{}.edi", case.replace([' ', ','], "-"));
        let input_file = stage(&router, &key, document.as_bytes()).await;

        let (status, response) = call(
            &router,
            "TestParsing",
            request(input_file.clone(), "JSON", "X12_850", "VERSION_4010"),
        )
        .await;
        let parsed = parsed_json(case, status, &response);
        let interchanges = parsed["interchanges"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: expected interchanges, got {parsed}"));
        assert_eq!(
            interchanges.len(),
            file.len(),
            "case {case:?}: expected {} interchanges, got {}",
            file.len(),
            interchanges.len()
        );

        for (index, (sender, isa12, groups)) in file.iter().enumerate() {
            let reported = &interchanges[index];
            assert_eq!(
                reported["ISA_06_SenderId"].as_str(),
                Some(*sender),
                "case {case:?}: interchange {index} must be the one {sender:?} sent, got \
                 {reported}"
            );
            assert_eq!(
                reported["ISA_12_Version"].as_str(),
                Some(*isa12),
                "case {case:?}: interchange {index} must carry its own ISA12, got {reported}"
            );
            let reported_groups = reported["functional_groups"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected groups, got {reported}"));
            assert_eq!(
                reported_groups.len(),
                groups.len(),
                "case {case:?}: interchange {index} must hold {} functional groups, got {}",
                groups.len(),
                reported_groups.len()
            );
            let count = groups.len().to_string();
            assert_eq!(
                reported["IEA_01_NumberOfIncludedFunctionalGroups"].as_str(),
                Some(count.as_str()),
                "case {case:?}: interchange {index} must report IEA01 as its own group \
                 count, got {reported}"
            );
        }

        // Splitting spans every interchange: one file per transaction set the WHOLE input
        // file holds, each carrying the envelope of the interchange ITS transaction set
        // sits in and reporting the one group and the one transaction set it really holds.
        let expected: Vec<(&str, &str)> = file
            .iter()
            .flat_map(|(sender, _, groups)| {
                groups.iter().flat_map(move |(_, transactions)| {
                    transactions
                        .iter()
                        .map(move |transaction| (*sender, transaction.control))
                })
            })
            .collect();
        let mut body = request(input_file, "JSON", "X12_850", "VERSION_4010");
        body["advancedOptions"] = json!({ "x12": { "splitOptions": { "splitBy": "TRANSACTION" } } });
        let (status, response) = call(&router, "TestParsing", body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 for a split parse, got {status} with body \
             {response:?}"
        );
        let contents = response["parsedSplitFileContents"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected parsedSplitFileContents, got {response:?}")
            });
        assert_eq!(
            contents.len(),
            expected.len(),
            "case {case:?}: expected {} split files, got {}",
            expected.len(),
            contents.len()
        );

        for (index, (sender, control)) in expected.iter().enumerate() {
            let text = contents[index].as_str().unwrap_or_default();
            let split: Value = serde_json::from_str(text).unwrap_or_else(|error| {
                panic!("case {case:?}: each split file must be a JSON document, got {error}: {text}")
            });
            let split_interchanges = split["interchanges"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: expected interchanges, got {split}"));
            assert_eq!(
                split_interchanges.len(),
                1,
                "case {case:?}: split file {index} must hold one interchange, got {}",
                split_interchanges.len()
            );
            let only = &split_interchanges[0];
            assert_eq!(
                only["ISA_06_SenderId"].as_str(),
                Some(*sender),
                "case {case:?}: split file {index} must carry the envelope of the \
                 interchange it came from, got {split}"
            );
            assert_eq!(
                only["IEA_01_NumberOfIncludedFunctionalGroups"].as_str(),
                Some("1"),
                "case {case:?}: split file {index} must report IEA01 as 1, got {split}"
            );
            assert_eq!(
                only["functional_groups"][0]["GE_01_NumberOfTransactionSetsIncluded"].as_str(),
                Some("1"),
                "case {case:?}: split file {index} must report GE01 as 1, got {split}"
            );
            assert_eq!(
                only["functional_groups"][0]["transactions"][0]
                    ["ST_02_TransactionSetControlNumber"]
                    .as_str(),
                Some(*control),
                "case {case:?}: split file {index} must hold transaction set {control:?}, \
                 got {split}"
            );
        }
    }
}
