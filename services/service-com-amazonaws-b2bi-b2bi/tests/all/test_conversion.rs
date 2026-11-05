// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TestConversion`, driven over the wire (awsJson1_0,
//! `x-amz-target: B2BI.TestConversion`) through the same router the emulator boots, with
//! Amazon S3 mounted beside b2bi on one engine so an interchange can be staged and read
//! back through `TestParsing` — which is how the round trip below is asserted.
//!
//! The rule under test: `TestConversion` answers the X12 interchange the B2Bi EDI document
//! its `source` carries holds. Each member of that document names the segment and the
//! POSITION of the element it carries, the envelopes nest as interchanges holding
//! functional groups holding transaction sets, every segment is delimited the way the X12
//! envelope declares — the component element separator the document's own ISA16 names — and
//! every trailer's first element counts what its envelope really holds. So a document this
//! service parsed out of an interchange converts back to that interchange, and a document
//! that is not the `fileFormat` it was declared as, or is not that shape at all, is refused
//! with the modelled `ValidationException`.
//!
//! Each test asserts that rule over a TABLE whose rows differ along what it generalises
//! over: the delimiter set, the number of functional groups and transaction sets, the
//! transaction set (one whose detail table this emulator loops and one whose it does not),
//! composite and absent elements, the X12 release, the source format, the order the
//! document's members arrive in and the trailers it leaves out.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{b2bi_router, call, error_code, given_bucket, s3, text};
// The X12 fixture builder this suite drives the WRITER over is the one the `test_parsing`
// suite drives the reader over, so both operations are asserted over the same interchanges.
use crate::test_parsing::{
    health_care_claim, interchange, purchase_order, segment, Delimiters, TransactionSet, STANDARD,
};

/// The bucket every interchange below is staged in.
const BUCKET: &str = "b2bi-test-conversion";

/// Store `content` at `key` and answer the `inputFile` that names it.
async fn stage(router: &MultiServiceRouter, key: &str, content: &str) -> Value {
    let (status, body) = s3(
        router,
        "PUT",
        &format!("/{BUCKET}/{key}"),
        content.as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "the interchange must be stored at {key:?}, got {status} with body {:?}",
        String::from_utf8_lossy(&body)
    );
    json!({ "bucketName": BUCKET, "key": key })
}

/// The canonical document `TestParsing` answers for the interchange staged at `key`, in
/// `file_format`.
async fn parse(
    router: &MultiServiceRouter,
    case: &str,
    key: &str,
    content: &str,
    file_format: &str,
    version: &str,
) -> String {
    let input_file = stage(router, key, content).await;
    let (status, body) = call(
        router,
        "TestParsing",
        json!({
            "inputFile": input_file,
            "fileFormat": file_format,
            "ediType": { "x12Details": { "transactionSet": "X12_850", "version": version } }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the interchange must parse, got {status} with body {body:?}"
    );
    text(&body, "parsedFileContent")
}

/// Convert the document `content`, declared as `file_format`, and answer `(status, body)`.
async fn convert(
    router: &MultiServiceRouter,
    file_format: &str,
    content: &str,
    advanced: Option<Value>,
) -> (u16, Value) {
    let mut target = json!({
        "fileFormat": "X12",
        "formatDetails": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
    });
    if let Some(advanced) = advanced {
        target["advancedOptions"] = advanced;
    }
    call(
        router,
        "TestConversion",
        json!({
            "source": { "fileFormat": file_format, "inputFile": { "fileContent": content } },
            "target": target
        }),
    )
    .await
}

/// The `convertedFileContent` of a successful conversion.
async fn converted(router: &MultiServiceRouter, case: &str, file_format: &str, content: &str) -> String {
    let (status, body) = convert(router, file_format, content, None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the document must convert, got {status} with body {body:?}"
    );
    text(&body, "convertedFileContent")
}

/// A transaction set whose detail table this emulator carries no loop for — `870` Order
/// Status Report is none of the transaction sets the reader's `DETAIL_LOOPS` names — so its
/// body segments are reported flat and have to be written back flat.
fn order_status<'a>(control: &'a str) -> TransactionSet<'a> {
    TransactionSet {
        code: "870",
        control,
        body: vec![
            vec!["BSR", "4", "2", control, "20240101"],
            vec!["HL", "1", "", "I"],
        ],
    }
}

/// One functional group of a table row: its functional identifier code and the transaction
/// sets it holds — the shape [`interchange`] takes its groups in.
type GroupRow<'a> = (&'a str, Vec<TransactionSet<'a>>);

/// One row of the round-trip table: the case, the delimiter set, the ISA12 the interchange
/// declares, the version the request names and the functional groups it holds.
type RoundTripRow<'a> = (&'a str, Delimiters, &'a str, &'a str, Vec<GroupRow<'a>>);

/// RULE: the interchange a document converts to is the interchange that document was parsed
/// out of — every segment, every element and every control count — so a conversion read
/// back through `TestParsing` answers the very document it was handed.
///
/// The rows differ along the delimiter set, the number of functional groups and transaction
/// sets, the transaction set (one whose detail table the reader loops, one whose it does
/// not), composite elements, elements the interchange leaves empty and the X12 release the
/// request names (which is what the name of ISA11 follows, so the writer must read an
/// element's POSITION and not its name).
#[tokio::test]
async fn a_parsed_interchange_converts_back_to_the_interchange_it_came_from() {
    let router = b2bi_router();
    given_bucket(&router, BUCKET).await;

    // (case, delimiters, the ISA12 the interchange declares, the version the request names,
    //  the functional groups and their transaction sets)
    let rows: Vec<RoundTripRow> = vec![
        (
            "one group holding one purchase order",
            STANDARD,
            "00401",
            "VERSION_4010",
            vec![("PO", vec![purchase_order("0001", "PO-1")])],
        ),
        (
            "two groups holding two transaction sets each",
            STANDARD,
            "00401",
            "VERSION_4010",
            vec![
                (
                    "PO",
                    vec![purchase_order("0001", "PO-1"), purchase_order("0002", "PO-2")],
                ),
                (
                    "HC",
                    vec![health_care_claim("0003"), health_care_claim("0004")],
                ),
            ],
        ),
        (
            "a transaction set carrying composite elements",
            STANDARD,
            "00401",
            "VERSION_4010",
            vec![("HC", vec![health_care_claim("0001")])],
        ),
        (
            "a transaction set whose detail table is reported flat",
            STANDARD,
            "00401",
            "VERSION_4010",
            vec![("AG", vec![order_status("0001")])],
        ),
        (
            "a release whose ISA11 is the repetition separator",
            STANDARD,
            "00501",
            "VERSION_5010",
            vec![("PO", vec![purchase_order("0001", "PO-1")])],
        ),
        (
            "delimiters of the sender's own choosing",
            ('|', ':', '\n'),
            "00401",
            "VERSION_4010",
            vec![("PO", vec![purchase_order("0001", "PO-1")])],
        ),
    ];

    for (index, (case, delimiters, isa_version, version, groups)) in rows.iter().enumerate() {
        let original = interchange(*delimiters, "SENDERID", "RECEIVERID", isa_version, groups);
        let document = parse(
            &router,
            case,
            &format!("round-trip/{index}.edi"),
            &original,
            "JSON",
            version,
        )
        .await;

        let written = converted(&router, case, "JSON", &document).await;

        // An interchange delimited the way X12 senders delimit one comes back byte for
        // byte: the writer puts every element where the document's member names say it
        // belongs, and the delimiters the envelope declares by position are those.
        if *delimiters == STANDARD {
            assert_eq!(
                written, original,
                "case {case:?}: expected the interchange it was parsed from, got {written:?}"
            );
        }

        // Whatever the input was delimited with, the written interchange holds the same
        // document: the one thing a conversion may not change.
        let again = parse(
            &router,
            case,
            &format!("round-trip/{index}-converted.edi"),
            &written,
            "JSON",
            version,
        )
        .await;
        assert_eq!(
            again, document,
            "case {case:?}: the converted interchange must parse back to the document it \
             was written from, got {again}"
        );
    }
}

/// RULE: the two source formats carry the same document, so the XML rendering of a document
/// converts to the interchange its JSON rendering converts to.
///
/// The rows differ along the transaction set and the number of transaction sets, so the
/// envelope nest, a composite element and a detail loop are each asserted through the XML
/// path too.
#[tokio::test]
async fn the_xml_rendering_of_a_document_converts_to_the_same_interchange() {
    let router = b2bi_router();
    given_bucket(&router, BUCKET).await;

    let rows: Vec<(&str, Vec<GroupRow>)> = vec![
        (
            "one purchase order",
            vec![("PO", vec![purchase_order("0001", "PO-1")])],
        ),
        (
            "a transaction set carrying composite elements",
            vec![("HC", vec![health_care_claim("0001")])],
        ),
        (
            "two groups holding two transaction sets each",
            vec![
                (
                    "PO",
                    vec![purchase_order("0001", "PO-1"), purchase_order("0002", "PO-2")],
                ),
                ("AG", vec![order_status("0003"), order_status("0004")]),
            ],
        ),
    ];

    for (index, (case, groups)) in rows.iter().enumerate() {
        let original = interchange(STANDARD, "SENDERID", "RECEIVERID", "00401", groups);
        let key = format!("xml/{index}.edi");
        let as_xml = parse(&router, case, &key, &original, "XML", "VERSION_4010").await;

        let written = converted(&router, case, "XML", &as_xml).await;
        assert_eq!(
            written, original,
            "case {case:?}: an XML document must convert to the interchange it was parsed \
             from, got {written:?}"
        );
    }
}

/// The ISA and GS every hand-written document below opens with, and the GE and IEA it
/// closes with: one interchange holding one functional group, with ISA02, ISA03 and ISA04
/// left out of the document so the empty element X12 encodes an absent element as is
/// asserted too.
const ENVELOPE: &str = "ISA*00****ZZ*SENDER*ZZ*RECEIVER*240101*1200*U*00401*000000001*0*P*>~\
                        GS*PO*SENDER*RECEIVER*20240101*1200*1*X*004010~";

/// The interchange `body` (the segments between ST and SE) is written as, inside
/// [`ENVELOPE`]: `included` is what SE01 has to count.
fn expected(body: &str, included: usize) -> String {
    format!("{ENVELOPE}ST*850*0001~{body}SE*{included}*0001~GE*1*1~IEA*1*000000001~")
}

/// The interchange control header's members, in the order the document's own rendering
/// writes them.
const ISA_MEMBERS: &str = r#""ISA_01_AuthorizationQualifier":"00",
    "ISA_05_SenderQualifier":"ZZ","ISA_06_SenderId":"SENDER",
    "ISA_07_ReceiverQualifier":"ZZ","ISA_08_ReceiverId":"RECEIVER",
    "ISA_09_Date":"240101","ISA_10_Time":"1200","ISA_11_RepetitionSeparator":"U",
    "ISA_12_Version":"00401","ISA_13_InterchangeControlNumber":"000000001",
    "ISA_14_AcknowledgmentRequested":"0","ISA_15_TestIndicator":"P",
    "ISA_16_ComponentSeparator":">""#;

/// The functional group header's members, in the same order.
const GS_MEMBERS: &str = r#""GS_01_FunctionalIdentifierCode":"PO",
    "GS_02_ApplicationSenderCode":"SENDER","GS_03_ApplicationReceiverCode":"RECEIVER",
    "GS_04_Date":"20240101","GS_05_Time":"1200","GS_06_GroupControlNumber":"1",
    "GS_07_ResponsibleAgencyCode":"X","GS_08_Version":"004010""#;

/// The transaction set header's members.
const ST_MEMBERS: &str = r#""ST_01_TransactionSetIdentifierCode":"850",
    "ST_02_TransactionSetControlNumber":"0001""#;

/// The two body segments of the transaction set below: one with an element it leaves out
/// (BEG04) and one COMPOSITE element, whose components the interchange separates with the
/// character ISA16 names.
const BODY_MEMBERS: &str = r#"{"segment_id":"BEG","BEG_01":"00","BEG_02":"SA",
      "BEG_03":"PO-1","BEG_05":"20240101"},
     {"segment_id":"HI","HI_01":{"HI_01_01":"BK","HI_01_02":"8901"}}"#;

/// One B2Bi EDI document holding [`BODY_MEMBERS`], with `trailers` closing its envelopes.
fn document(trailers: (&str, &str, &str)) -> String {
    let (se, ge, iea) = trailers;
    format!(
        "{{\"interchanges\":[{{{ISA_MEMBERS},\"functional_groups\":[{{{GS_MEMBERS},\
         \"transactions\":[{{{ST_MEMBERS},\"segments\":[{BODY_MEMBERS}]{se}}}]{ge}}}]{iea}}}]}}"
    )
}

/// RULE: a member's place in the interchange comes from the segment and position its NAME
/// carries, not from where it sits in the document, and a trailer's first element counts
/// what its envelope was written with — the number of segments for SE01, of transaction sets
/// for GE01, of functional groups for IEA01 — whatever number the document itself carried.
///
/// The rows differ along the order the members arrive in, the trailers the document leaves
/// out, a count that disagrees with what the document holds, an absent ISA16 and a detail
/// LOOP entry standing in for the segments it holds.
#[tokio::test]
async fn an_elements_place_comes_from_its_name_and_a_trailer_counts_what_was_written() {
    let router = b2bi_router();

    let correct = document((
        ",\"SE_01_NumberOfIncludedSegments\":\"4\",\
          \"SE_02_TransactionSetControlNumber\":\"0001\"",
        ",\"GE_01_NumberOfTransactionSetsIncluded\":\"1\",\"GE_02_GroupControlNumber\":\"1\"",
        ",\"IEA_01_NumberOfIncludedFunctionalGroups\":\"1\",\
          \"IEA_02_InterchangeControlNumber\":\"000000001\"",
    ));
    // The same document with every envelope's trailer left out, and with counts that
    // disagree with what it holds.
    let without_trailers = document((
        "",
        ",\"GE_02_GroupControlNumber\":\"1\"",
        ",\"IEA_02_InterchangeControlNumber\":\"000000001\"",
    ));
    let wrong_counts = document((
        ",\"SE_01_NumberOfIncludedSegments\":\"99\",\
          \"SE_02_TransactionSetControlNumber\":\"0001\"",
        ",\"GE_01_NumberOfTransactionSetsIncluded\":\"7\",\"GE_02_GroupControlNumber\":\"1\"",
        ",\"IEA_01_NumberOfIncludedFunctionalGroups\":\"5\",\
          \"IEA_02_InterchangeControlNumber\":\"000000001\"",
    ));
    // The members of the interchange, the group, the transaction set and each of its
    // segments in the reverse of the order the document's own rendering writes them. The
    // `segments` LIST keeps document order: a list is the order of the body, while a
    // member's name is what places its element.
    let reversed = format!(
        "{{\"interchanges\":[{{\
         \"IEA_02_InterchangeControlNumber\":\"000000001\",\
         \"IEA_01_NumberOfIncludedFunctionalGroups\":\"1\",\
         \"functional_groups\":[{{\
         \"GE_02_GroupControlNumber\":\"1\",\"GE_01_NumberOfTransactionSetsIncluded\":\"1\",\
         \"transactions\":[{{\
         \"SE_02_TransactionSetControlNumber\":\"0001\",\
         \"SE_01_NumberOfIncludedSegments\":\"4\",\
         \"segments\":[{{\"BEG_05\":\"20240101\",\"BEG_03\":\"PO-1\",\"BEG_02\":\"SA\",\
         \"BEG_01\":\"00\",\"segment_id\":\"BEG\"}},\
         {{\"HI_01\":{{\"HI_01_02\":\"8901\",\"HI_01_01\":\"BK\"}},\
         \"segment_id\":\"HI\"}}],\
         \"ST_02_TransactionSetControlNumber\":\"0001\",\
         \"ST_01_TransactionSetIdentifierCode\":\"850\"}}],{GS_MEMBERS}}}],{ISA_MEMBERS}}}]}}"
    );
    // An interchange whose ISA16 the document leaves out: the control header still carries
    // the sixteen elements it is defined to have, ISA16 being the component separator
    // composites are written with.
    let no_component_separator = format!(
        "{{\"interchanges\":[{{\"ISA_01_AuthorizationQualifier\":\"00\",\
         \"ISA_05_SenderQualifier\":\"ZZ\",\"ISA_06_SenderId\":\"SENDER\",\
         \"ISA_07_ReceiverQualifier\":\"ZZ\",\"ISA_08_ReceiverId\":\"RECEIVER\",\
         \"ISA_09_Date\":\"240101\",\"ISA_10_Time\":\"1200\",\
         \"ISA_11_RepetitionSeparator\":\"U\",\"ISA_12_Version\":\"00401\",\
         \"ISA_13_InterchangeControlNumber\":\"000000001\",\
         \"ISA_14_AcknowledgmentRequested\":\"0\",\"ISA_15_TestIndicator\":\"P\",\
         \"functional_groups\":[{{{GS_MEMBERS},\"transactions\":[{{{ST_MEMBERS},\
         \"segments\":[{BODY_MEMBERS}]}}]}}]}}]}}"
    );
    // A transaction set whose repeating detail table arrived as ONE loop entry holding one
    // object per occurrence, each occurrence carrying the segments inside it.
    let detail_loop = format!(
        "{{\"interchanges\":[{{{ISA_MEMBERS},\"functional_groups\":[{{{GS_MEMBERS},\
         \"transactions\":[{{{ST_MEMBERS},\"segments\":[\
         {{\"segment_id\":\"BEG\",\"BEG_01\":\"00\",\"BEG_02\":\"SA\",\"BEG_03\":\"PO-1\",\
         \"BEG_05\":\"20240101\"}},\
         {{\"PO1-8000_loop\":[\
         {{\"segment_id\":\"PO1\",\"PO1_01\":\"1\",\"PO1_02\":\"10\",\"PO1_03\":\"EA\",\
         \"segments\":[{{\"segment_id\":\"PID\",\"PID_01\":\"F\",\"PID_05\":\"WIDGET\"}}]}},\
         {{\"segment_id\":\"PO1\",\"PO1_01\":\"2\",\"PO1_02\":\"5\",\"PO1_03\":\"EA\"}}]}},\
         {{\"segment_id\":\"CTT\",\"CTT_01\":\"2\"}}]}}]}}]}}]}}"
    );

    let body = "BEG*00*SA*PO-1**20240101~HI*BK>8901~";
    let looped = "BEG*00*SA*PO-1**20240101~PO1*1*10*EA~PID*F****WIDGET~PO1*2*5*EA~CTT*2~";

    // (case, the document, the interchange it has to be written as)
    let rows: Vec<(&str, &str, String)> = vec![
        ("every member in document order", &correct, expected(body, 4)),
        ("every member in reverse order", &reversed, expected(body, 4)),
        ("no trailer counts at all", &without_trailers, expected(body, 4)),
        ("counts that disagree with the document", &wrong_counts, expected(body, 4)),
        (
            "no component element separator",
            &no_component_separator,
            expected(body, 4),
        ),
        ("a detail loop entry", &detail_loop, expected(looped, 7)),
    ];

    for (case, content, expected) in rows {
        let written = converted(&router, case, "JSON", content).await;
        assert_eq!(
            written, expected,
            "case {case:?}: expected {expected:?}, got {written:?}"
        );
    }
}

/// RULE: content that is its declared format but is not a B2Bi EDI document is refused with
/// the modelled `ValidationException` (HTTP 400) naming what is not one, because there is no
/// interchange to answer with — never a success carrying an interchange the document does
/// not hold.
///
/// The rows differ along which part of the document shape is missing or unusable: the
/// `interchanges` member, an interchange, each of the three envelope headers, a segment's
/// `segment_id`, an id that is not one, an ISA16 that cannot delimit components, an element
/// value or composite component carrying one of the three delimiters, a value that is not an
/// element's, a body segment that is an envelope header (so the segments would not nest as
/// an interchange), a member naming a position past the elements a segment is written with, a
/// document nested deeper than a conversion reads (in both formats) and a document past the
/// size limit.
#[tokio::test]
async fn a_document_that_is_not_a_b2bi_edi_document_is_refused() {
    let router = b2bi_router();

    let isa = json!({
        "ISA_01_AuthorizationQualifier": "00",
        "ISA_06_SenderId": "SENDER",
        "ISA_13_InterchangeControlNumber": "000000001",
        "ISA_16_ComponentSeparator": ">"
    });
    /// One interchange carrying `members` beside its ISA elements.
    fn with_isa(isa: &Value, members: Value) -> String {
        let mut interchange = isa.clone();
        for (key, value) in members.as_object().expect("an object").clone() {
            interchange[key] = value;
        }
        json!({ "interchanges": [interchange] }).to_string()
    }

    let oversized = "a".repeat(256 * 1024 + 1);
    let rows: Vec<(&str, &str, String, &str)> = vec![
        (
            "a JSON document that carries no interchanges",
            "JSON",
            json!({ "transactionSets": [] }).to_string(),
            "carries no interchanges member",
        ),
        (
            "an interchanges member that holds none",
            "JSON",
            json!({ "interchanges": [] }).to_string(),
            "holds no interchange to convert",
        ),
        (
            "an interchange with no ISA element",
            "JSON",
            json!({ "interchanges": [{ "functional_groups": [] }] }).to_string(),
            "carries no interchange control header",
        ),
        (
            "a functional group with no GS element",
            "JSON",
            with_isa(&isa, json!({ "functional_groups": [{ "transactions": [] }] })),
            "carries no functional group header",
        ),
        (
            "a transaction set with no ST element",
            "JSON",
            with_isa(
                &isa,
                json!({ "functional_groups": [{
                    "GS_01_FunctionalIdentifierCode": "PO",
                    "transactions": [{ "segments": [] }]
                }] }),
            ),
            "carries no transaction set header",
        ),
        (
            "a body segment with no segment_id",
            "JSON",
            with_isa(&isa, json!({ "segments": [{ "BEG_01": "00" }] })),
            "carries no segment_id",
        ),
        (
            "a segment id that is not an X12 segment identifier",
            "JSON",
            with_isa(&isa, json!({ "segments": [{ "segment_id": "1BEG" }] })),
            "is not an X12 segment identifier",
        ),
        (
            "an ISA16 that cannot delimit the interchange's components",
            "JSON",
            json!({ "interchanges": [{
                "ISA_01_AuthorizationQualifier": "00",
                "ISA_16_ComponentSeparator": "*"
            }] })
            .to_string(),
            "cannot be one",
        ),
        (
            "an element whose value is a list",
            "JSON",
            with_isa(
                &isa,
                json!({ "segments": [{ "segment_id": "BEG", "BEG_01": ["00", "01"] }] }),
            ),
            "carries a list of values",
        ),
        (
            "a body segment that is an envelope header",
            "JSON",
            with_isa(&isa, json!({ "segments": [{ "segment_id": "GS" }] })),
            "could not be parsed",
        ),
        (
            "an element value carrying the element separator",
            "JSON",
            with_isa(
                &isa,
                json!({ "segments": [{ "segment_id": "BEG", "BEG_01": "A*B" }] }),
            ),
            "is a character no X12 element's value is made of",
        ),
        (
            "an element value carrying the segment terminator",
            "JSON",
            with_isa(
                &isa,
                json!({ "segments": [{ "segment_id": "BEG", "BEG_01": "A~B" }] }),
            ),
            "is a character no X12 element's value is made of",
        ),
        (
            "a composite component carrying the component separator",
            "JSON",
            with_isa(
                &isa,
                json!({ "segments": [{
                    "segment_id": "HI",
                    "HI_01": { "HI_01_01": "A>B", "HI_01_02": "C" }
                }] }),
            ),
            "is a character no X12 element's value is made of",
        ),
        (
            "an element position past the elements a segment is written with",
            "JSON",
            with_isa(
                &isa,
                json!({ "segments": [{ "segment_id": "BEG", "BEG_101": "00" }] }),
            ),
            "names a position past",
        ),
        (
            "a document nested deeper than a conversion reads",
            "JSON",
            format!("{}{}", "[".repeat(70), "]".repeat(70)),
            "nests deeper than",
        ),
        (
            "an XML document nested deeper than a conversion reads",
            "XML",
            format!(
                "{}{}",
                "<nest>".repeat(70),
                "</nest>".repeat(70)
            ),
            "nests deeper than",
        ),
        (
            "an XML document that carries no interchanges",
            "XML",
            "<transactionSets><set>1</set></transactionSets>".to_string(),
            "carries no interchanges member",
        ),
        (
            "a source document past the size limit",
            "JSON",
            oversized,
            "more than",
        ),
    ];

    for (case, file_format, content, expected) in rows {
        let (status, body) = convert(&router, file_format, &content, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(expected),
            "case {case:?}: expected a refusal saying {expected:?}, got {message:?}"
        );
    }
}

/// RULE: a configured element requirement rule is evaluated over the interchange the
/// conversion WROTE and reported in `validationMessages` — one message per occurrence of a
/// segment that carries no value for an element made MANDATORY — while a rule keyed by an
/// X12 data element id, which no data element dictionary here could apply, is refused
/// rather than reported as a validation that passed.
///
/// The rows differ along the element the rule names (one the interchange carries, one it
/// leaves empty, one its header does not reach), the requirement and the kind of rule.
#[tokio::test]
async fn configured_element_requirement_rules_are_reported_over_the_written_interchange() {
    let router = b2bi_router();

    let content = document((
        ",\"SE_01_NumberOfIncludedSegments\":\"4\",\
          \"SE_02_TransactionSetControlNumber\":\"0001\"",
        ",\"GE_01_NumberOfTransactionSetsIncluded\":\"1\",\"GE_02_GroupControlNumber\":\"1\"",
        ",\"IEA_01_NumberOfIncludedFunctionalGroups\":\"1\",\
          \"IEA_02_InterchangeControlNumber\":\"000000001\"",
    ));

    // (case, the configured rules, how many validation messages the conversion reports)
    let rows: Vec<(&str, Value, usize)> = vec![
        (
            "an element the written interchange carries",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-03", "requirement": "MANDATORY" } }]),
            0,
        ),
        (
            "an element the written interchange leaves empty",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "MANDATORY" } }]),
            1,
        ),
        (
            "an envelope element the header does not carry",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "ST-03", "requirement": "MANDATORY" } }]),
            1,
        ),
        (
            "the same element made optional",
            json!([{ "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "OPTIONAL" } }]),
            0,
        ),
        (
            "two rules at once",
            json!([
                { "elementRequirementValidationRule": { "elementPosition": "BEG-04", "requirement": "MANDATORY" } },
                { "elementRequirementValidationRule": { "elementPosition": "ST-03", "requirement": "MANDATORY" } }
            ]),
            2,
        ),
    ];

    for (case, rules, expected) in rows {
        let advanced = json!({ "x12": { "validationOptions": { "validationRules": rules } } });
        let (status, body) = convert(&router, "JSON", &content, Some(advanced)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body:?}"
        );
        let messages = body["validationMessages"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default();
        assert_eq!(
            messages.len(),
            expected,
            "case {case:?}: expected {expected} validation messages, got {messages:?}"
        );
    }

    // A rule keyed by a four-digit X12 data element id is refused, not reported as a
    // validation that passed.
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
        let advanced = json!({ "x12": { "validationOptions": { "validationRules": rules } } });
        let (status, body) = convert(&router, "JSON", &content, Some(advanced)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }
}

/// RULE: the `segments` a transaction set holds are written between its ST and SE in
/// document order, so a document holding segments no fixture above carries converts to an
/// interchange whose segments are those, in that order.
///
/// This is the property the round trip asserts over parsed documents, asserted over a
/// document written by hand: the rows differ along the number of segments and the number of
/// elements each one carries, including a segment that carries none at all.
#[tokio::test]
async fn a_transaction_sets_segments_are_written_in_document_order() {
    let router = b2bi_router();

    // (case, the body segments of the transaction set, the segments they are written as)
    let rows: Vec<(&str, Value, Vec<Vec<&str>>)> = vec![
        (
            "one segment carrying no element",
            json!([{ "segment_id": "LS" }]),
            vec![vec!["LS"]],
        ),
        (
            "three segments in document order",
            json!([
                { "segment_id": "N1", "N1_01": "ST", "N1_02": "WAREHOUSE" },
                { "segment_id": "N3", "N3_01": "1 MAIN ST" },
                { "segment_id": "N4", "N4_01": "SEATTLE", "N4_02": "WA", "N4_03": "98101" }
            ]),
            vec![
                vec!["N1", "ST", "WAREHOUSE"],
                vec!["N3", "1 MAIN ST"],
                vec!["N4", "SEATTLE", "WA", "98101"],
            ],
        ),
        (
            "a segment whose first elements are left out",
            json!([{ "segment_id": "PID", "PID_05": "WIDGET" }]),
            vec![vec!["PID", "", "", "", "", "WIDGET"]],
        ),
    ];

    for (case, segments, written_as) in rows {
        let content = format!(
            "{{\"interchanges\":[{{{ISA_MEMBERS},\"functional_groups\":[{{{GS_MEMBERS},\
             \"transactions\":[{{{ST_MEMBERS},\"segments\":{segments}}}]}}]}}]}}"
        );
        let written = converted(&router, case, "JSON", &content).await;

        let body: String = written_as
            .iter()
            .map(|fields| segment(STANDARD, fields))
            .collect();
        let expected = expected(&body, written_as.len() + 2);
        assert_eq!(
            written, expected,
            "case {case:?}: expected {expected:?}, got {written:?}"
        );
    }
}
