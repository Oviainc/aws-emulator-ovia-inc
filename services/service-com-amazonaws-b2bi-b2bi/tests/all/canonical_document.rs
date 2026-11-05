// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CANONICAL EDI-as-JSON document this service answers with,
//! driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same
//! router the emulator boots, with the emulator's own Amazon S3 service mounted beside
//! b2bi — the EDI documents these operations read are S3 objects.
//!
//! The rule under test: the document an EDI interchange is canonicalized into names each
//! envelope element `<SEGMENT>_<position>_<canonical element name>` — the service's own
//! short form of the ASC X12 element titles, `ISA_06_SenderId` and not
//! `ISA_06_InterchangeSenderId` — and groups a transaction set's repeating detail segment
//! into ONE `<SEGMENT>-8000_loop` entry of that transaction's `segments`, holding one
//! object per occurrence. Every operation that answers the canonical document answers the
//! same member names: `TestParsing`'s `parsedFileContent` and the `mappingTemplate`
//! `CreateStarterMappingTemplate` derives from a sample, which carries the sample's
//! envelope values verbatim and each transaction-set business element under the
//! `PLACEHOLDER: ` marker over the value the sample held there.
//!
//! Each test states that rule over a TABLE whose rows differ along what it generalises
//! over: the element (and so its canonical name), the transaction set (and so the segment
//! its detail loop repeats), how many occurrences that loop has, whether an occurrence
//! holds subordinate segments, and the sample's own values.

use serde_json::{json, Value};

use crate::edi_parsing::{given_object, BUCKET};
use crate::onboarding_loop::{b2bi_router, call};
use crate::test_parsing::body_segments_in_order;

/// An X12 interchange carrying one transaction set of `transaction_set` whose body is
/// `body`, written with the classic delimiters (`*`, `>`, `~`).
///
/// The envelope values are fixed per call so a row can assert which of them the canonical
/// document carries and under which name: `sender` is ISA06 and `control` is ISA13.
fn interchange(transaction_set: &str, sender: &str, control: &str, body: &[&str]) -> String {
    let mut document = format!(
        "ISA*00*          *00*          *ZZ*{sender:<15}*ZZ*RECEIVER       \
         *240101*1200*U*00401*{control}*0*P*>~\
         GS*PO*{sender}*RECEIVER*20240101*1200*1*X*004010~\
         ST*{transaction_set}*0001~"
    );
    for segment in body {
        document.push_str(segment);
        document.push('~');
    }
    document.push_str(&format!(
        "SE*{}*0001~GE*1*1~IEA*1*{control}~",
        body.len() + 2
    ));
    document
}

/// The parsed document `TestParsing` answers for the object at `key`, as JSON.
async fn parsed_document(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key: &str,
    transaction_set: &str,
) -> Value {
    let (status, body) = call(
        router,
        "TestParsing",
        json!({
            "inputFile": { "bucketName": BUCKET, "key": key },
            "fileFormat": "JSON",
            "ediType": {
                "x12Details": { "transactionSet": transaction_set, "version": "VERSION_4010" }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: TestParsing must parse the sample, got {status} {body}"
    );
    let content = body["parsedFileContent"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a parsedFileContent, got {body}"));
    serde_json::from_str(content).unwrap_or_else(|failure| {
        panic!("case {case:?}: parsedFileContent must be a JSON document: {failure}: {content}")
    })
}

/// The `mappingTemplate` `CreateStarterMappingTemplate` derives from the sample at `key`,
/// as JSON.
async fn derived_template(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    key: &str,
    transaction_set: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreateStarterMappingTemplate",
        json!({
            "mappingType": "JSONATA",
            "templateDetails": {
                "x12": { "transactionSet": transaction_set, "version": "VERSION_4010" }
            },
            "outputSampleLocation": { "bucketName": BUCKET, "key": key }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the template must be derived from the sample, got {status} {body}"
    );
    let template = body["mappingTemplate"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a mappingTemplate, got {body}"));
    serde_json::from_str(template).unwrap_or_else(|failure| {
        panic!("case {case:?}: a JSONATA template must be a JSON document: {failure}: {template}")
    })
}

/// The one loop entry of `segments`, as `(member name, occurrences)`, or `None` when no
/// entry of the list is a loop.
fn detail_loop(segments: &[Value]) -> Option<(String, Vec<Value>)> {
    segments.iter().find_map(|entry| {
        let members = entry.as_object()?;
        let (key, value) = members.iter().next()?;
        if members.len() != 1 || !key.ends_with("_loop") {
            return None;
        }
        Some((key.clone(), value.as_array().cloned().unwrap_or_default()))
    })
}

/// RULE: every envelope element of the canonical document is keyed
/// `<SEGMENT>_<position>_<canonical element name>`, and the canonical name is the
/// service's own short form of the ASC X12 element title.
///
/// The rows are the elements themselves — one per envelope segment of the interchange,
/// including elements this emulator's recorded canonical documents do not cite
/// (`ISA_05_SenderQualifier`, `ISA_09_Date`, `GS_02_ApplicationSenderCode`,
/// `IEA_02_InterchangeControlNumber`) — and each row's value is the one the sample
/// interchange carries at that position, so the assertion is about the NAME the value
/// arrives under. Both operations that answer the canonical document are asserted with the
/// same table: a starter template describes the very document the reader builds, so the two
/// cannot name an element differently.
#[tokio::test]
async fn every_envelope_element_carries_its_canonical_member_name() {
    let router = b2bi_router();

    let sample = interchange("850", "SENDER", "000000007", &["BEG*00*SA*PO-7**20240101"]);
    let key = "canonical/envelope-850.edi";
    given_object(&router, key, &sample).await;

    // Each row: the member name and the value the sample carries at that position. The
    // ISA11 name is release-dependent (release 004010 here, below the release where the
    // Repetition Separator takes position 11).
    let interchange_elements: &[(&str, &str)] = &[
        ("ISA_01_AuthorizationQualifier", "00"),
        ("ISA_03_SecurityQualifier", "00"),
        ("ISA_05_SenderQualifier", "ZZ"),
        ("ISA_06_SenderId", "SENDER         "),
        ("ISA_07_ReceiverQualifier", "ZZ"),
        ("ISA_08_ReceiverId", "RECEIVER       "),
        ("ISA_09_Date", "240101"),
        ("ISA_10_Time", "1200"),
        ("ISA_11_InterchangeControlStandardsId", "U"),
        ("ISA_12_Version", "00401"),
        ("ISA_13_InterchangeControlNumber", "000000007"),
        ("ISA_14_AcknowledgmentRequested", "0"),
        ("ISA_15_TestIndicator", "P"),
        ("ISA_16_ComponentSeparator", ">"),
        ("IEA_01_NumberOfIncludedFunctionalGroups", "1"),
        ("IEA_02_InterchangeControlNumber", "000000007"),
    ];
    let group_elements: &[(&str, &str)] = &[
        ("GS_01_FunctionalIdentifierCode", "PO"),
        ("GS_02_ApplicationSenderCode", "SENDER"),
        ("GS_03_ApplicationReceiverCode", "RECEIVER"),
        ("GS_04_Date", "20240101"),
        ("GS_05_Time", "1200"),
        ("GS_06_GroupControlNumber", "1"),
        ("GS_07_ResponsibleAgencyCode", "X"),
        ("GS_08_Version", "004010"),
        ("GE_01_NumberOfTransactionSetsIncluded", "1"),
        ("GE_02_GroupControlNumber", "1"),
    ];
    let transaction_elements: &[(&str, &str)] = &[
        ("ST_01_TransactionSetIdentifierCode", "850"),
        ("ST_02_TransactionSetControlNumber", "0001"),
        ("SE_01_NumberOfIncludedSegments", "3"),
        ("SE_02_TransactionSetControlNumber", "0001"),
    ];

    let parsed = parsed_document(&router, "parsed document", key, "X12_850").await;
    let template = derived_template(&router, "derived template", key, "X12_850").await;

    for (document_case, document) in [("parsed document", &parsed), ("derived template", &template)]
    {
        let reported = &document["interchanges"][0];
        let group = &reported["functional_groups"][0];
        let transaction = &group["transactions"][0];
        for (scope, object, elements) in [
            ("interchange", reported, interchange_elements),
            ("functional group", group, group_elements),
            ("transaction set", transaction, transaction_elements),
        ] {
            for (member, value) in elements {
                assert_eq!(
                    object[member].as_str(),
                    Some(*value),
                    "case {document_case:?}: the {scope}'s {member} must carry {value:?}, got \
                     {object}"
                );
            }
        }
    }

    // The canonical names REPLACE the X12 titles: a member named by the title is absent
    // from the whole document, not carried beside the canonical one.
    let rendered = format!("{parsed}{template}");
    for title in [
        "InterchangeSenderId",
        "InterchangeReceiverId",
        "AuthorizationInformationQualifier",
        "InterchangeIdQualifier",
        "UsageIndicator",
        "VersionReleaseIndustryIdentifierCode",
        "ApplicationSendersCode",
    ] {
        assert!(
            !rendered.contains(title),
            "the canonical document must not name an element {title:?}, got {rendered}"
        );
    }
}

/// One row of the detail-loop table: the case, the transaction set code, the
/// `X12TransactionSet` the request names, the body segments of the sample, the loop member
/// name the canonical document must carry (`None` for a transaction set whose detail loop
/// this emulator does not carry), how many occurrences that loop has, and the segment ids
/// the entries of `segments` must be, loops flattened.
type LoopCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    Option<&'a str>,
    usize,
    Vec<&'a str>,
);

/// RULE: a transaction set whose detail table repeats carries that repetition as ONE entry
/// of the transaction's `segments`, keyed `<SEGMENT>-8000_loop`, holding one object per
/// occurrence — and the segments the X12 standard places in the summary table stay beside
/// that loop rather than inside its last occurrence.
///
/// The rows differ along the transaction set (and so the segment its detail loop repeats),
/// the number of occurrences, whether an occurrence holds subordinate segments, whether a
/// summary segment follows the loop, and whether this emulator carries a detail loop for
/// that transaction set at all.
#[tokio::test]
async fn a_detail_loop_is_grouped_as_one_entry_per_occurrence() {
    let router = b2bi_router();

    let cases: Vec<LoopCase> = vec![
        (
            "850 purchase order with one line item",
            "850",
            "X12_850",
            vec!["BEG*00*SA*PO-1**20240101", "PO1*1*10*EA*9.99**VP*WIDGET-A"],
            Some("PO1-8000_loop"),
            1,
            vec!["BEG", "PO1"],
        ),
        (
            "850 purchase order with two line items, each with a description",
            "850",
            "X12_850",
            vec![
                "BEG*00*SA*PO-2**20240101",
                "PO1*1*10*EA*9.99**VP*WIDGET-A",
                "PID*F****First widget",
                "PO1*2*20*EA*8.88**VP*WIDGET-B",
                "PID*F****Second widget",
                "CTT*2",
            ],
            Some("PO1-8000_loop"),
            2,
            vec!["BEG", "PO1", "PID", "PO1", "PID", "CTT"],
        ),
        (
            "810 invoice, whose detail table repeats its baseline item data",
            "810",
            "X12_810",
            vec![
                "BIG*20240201*INV-9*20240101*PO-2",
                "IT1*1*5*EA*4.44**VP*WIDGET-A",
                "IT1*2*6*EA*3.33**VP*WIDGET-B",
                "TDS*4000",
            ],
            Some("IT1-8000_loop"),
            2,
            vec!["BIG", "IT1", "IT1", "TDS"],
        ),
        (
            "856 ship notice, whose detail table repeats its hierarchical levels",
            "856",
            "X12_856",
            vec![
                "BSN*00*SHIP-1*20240101*1200",
                "HL*1**S",
                "HL*2*1*O",
                "CTT*2",
            ],
            Some("HL-8000_loop"),
            2,
            vec!["BSN", "HL", "HL", "CTT"],
        ),
        (
            "997 acknowledgment, whose detail table repeats its transaction set responses",
            "997",
            "X12_997",
            vec!["AK1*PO*1", "AK2*850*0001", "AK2*850*0002", "AK9*A*2*2*2"],
            Some("AK2-8000_loop"),
            2,
            vec!["AK1", "AK2", "AK2", "AK9"],
        ),
        (
            "870 order status report, a transaction set with no detail loop here",
            "870",
            "X12_870",
            vec!["BSR*4*PO*STATUS-1*20240101", "HL*1**S", "HL*2*1*O"],
            None,
            0,
            vec!["BSR", "HL", "HL"],
        ),
    ];

    for (case, code, transaction_set, body, loop_member, occurrences, segment_ids) in cases {
        let key = format!("canonical/loop-{code}-{occurrences}.edi");
        let sample = interchange(code, "SENDER", "000000001", &body);
        given_object(&router, &key, &sample).await;

        let parsed = parsed_document(&router, case, &key, transaction_set).await;
        let segments = parsed["interchanges"][0]["functional_groups"][0]["transactions"][0]
            ["segments"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the transaction's segments"));

        match loop_member {
            Some(expected) => {
                let (member, items) = detail_loop(&segments).unwrap_or_else(|| {
                    panic!("case {case:?}: expected a {expected} entry, got {segments:?}")
                });
                assert_eq!(
                    member, expected,
                    "case {case:?}: the loop must be keyed {expected}, got {member:?}"
                );
                assert_eq!(
                    items.len(),
                    occurrences,
                    "case {case:?}: expected {occurrences} occurrences, got {items:?}"
                );
                // Each occurrence carries its own opening segment, so a mapping addresses
                // the second line item as the second entry of the loop.
                let opening = expected.split('-').next().unwrap_or_default();
                for (index, occurrence) in items.iter().enumerate() {
                    assert_eq!(
                        occurrence["segment_id"].as_str(),
                        Some(opening),
                        "case {case:?}: occurrence {index} must be a {opening} segment, got \
                         {occurrence}"
                    );
                }
            }
            None => assert!(
                detail_loop(&segments).is_none(),
                "case {case:?}: a transaction set with no detail loop here reports its segments \
                 flat, got {segments:?}"
            ),
        }

        // Whatever the grouping, every segment of the body is reported once, in document
        // order.
        let reported: Vec<String> = body_segments_in_order(&segments)
            .iter()
            .map(|segment| segment["segment_id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, segment_ids,
            "case {case:?}: expected the body segments {segment_ids:?}, got {reported:?}"
        );
    }
}

/// One row of the sample table: the case, the sample's sender (ISA06), its interchange
/// control number (ISA13), its body segments, and the (member, sample value) pairs of the
/// transaction-set business fields the derived template must mark.
type SampleCase<'a> = (
    &'a str,
    &'a str,
    &'a str,
    Vec<&'a str>,
    Vec<(&'a str, &'a str)>,
);

/// RULE: a starter mapping template derived from `outputSampleLocation` is the canonical
/// document of THAT sample — its envelope values carried through verbatim, and every
/// transaction-set business element replaced by the `PLACEHOLDER: ` marker over the value
/// the sample carried there — so a caller's mapping addresses the members its own documents
/// really hold.
///
/// The rows differ along the sample's envelope values, how many line items it holds (and so
/// how many loop occurrences the template describes) and which business elements it fills.
#[tokio::test]
async fn a_template_derived_from_a_sample_echoes_its_envelope_and_marks_its_business_fields() {
    let router = b2bi_router();

    let cases: Vec<SampleCase> = vec![
        (
            "one line item",
            "SENDER",
            "000000001",
            vec!["BEG*00*SA*PO123456**20240101", "PO1*1*10*EA*9.99**VP*WIDGET-A"],
            vec![("BEG_03", "PO123456"), ("PO1_07", "WIDGET-A")],
        ),
        (
            "another sender, another control number, two line items",
            "ACME",
            "000000042",
            vec![
                "BEG*05*RE*PO-RUSH-9**20240202",
                "PO1*1*1*CA*1.50**VP*BOLT-M6*VN*SUP-77",
                "PO1*2*2*CA*2.50**VP*NUT-M6*VN*SUP-78",
            ],
            vec![
                ("BEG_03", "PO-RUSH-9"),
                ("PO1_07", "BOLT-M6"),
                ("PO1_09", "SUP-77"),
            ],
        ),
    ];

    for (case, sender, control, body, marked) in cases {
        let key = format!("canonical/sample-{control}.edi");
        let sample = interchange("850", sender, control, &body);
        given_object(&router, &key, &sample).await;

        let template = derived_template(&router, case, &key, "X12_850").await;
        let reported = &template["interchanges"][0];

        // The envelope is the sample's own, verbatim: the caller does not map it. ISA06 is
        // the fixed-width field X12 pads, so it is compared trimmed.
        assert_eq!(
            reported["ISA_06_SenderId"].as_str().map(str::trim),
            Some(sender),
            "case {case:?}: ISA06 must carry the sample's sender, got {reported}"
        );
        assert_eq!(
            reported["ISA_13_InterchangeControlNumber"].as_str(),
            Some(control),
            "case {case:?}: ISA13 must carry the sample's control number, got {reported}"
        );

        // …and every business element of the transaction set is the marker over the
        // sample's own value.
        let transaction = &reported["functional_groups"][0]["transactions"][0];
        let segments = transaction["segments"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: expected the transaction's segments"));
        let flattened = body_segments_in_order(&segments);
        for (member, value) in &marked {
            let expected = format!("PLACEHOLDER: {value}");
            let found = flattened
                .iter()
                .filter_map(|segment| segment[member].as_str())
                .next();
            assert_eq!(
                found,
                Some(expected.as_str()),
                "case {case:?}: {member} must be marked {expected:?}, got {flattened:?}"
            );
        }
        // The line items the sample holds are the occurrences the template describes.
        let (_, occurrences) = detail_loop(&segments)
            .unwrap_or_else(|| panic!("case {case:?}: expected the detail loop, got {segments:?}"));
        let line_items = body
            .iter()
            .filter(|segment| segment.starts_with("PO1*"))
            .count();
        assert_eq!(
            occurrences.len(),
            line_items,
            "case {case:?}: expected {line_items} loop occurrences, got {occurrences:?}"
        );
    }
}
