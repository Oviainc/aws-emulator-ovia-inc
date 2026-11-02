// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ONE X12 READER this service reads an EDI document through,
//! driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same
//! router the emulator boots, with the emulator's own Amazon S3 service mounted beside
//! b2bi — every EDI document these operations read is an S3 object.
//!
//! The rule under test: the operations that read an EDI document — `TestParsing`,
//! `CreateStarterMappingTemplate`'s sample-driven path and `StartTransformerJob`'s
//! `inputConversion` — read it through the SAME reader
//! (`ops::test_parsing::parse_x12`), so one file is parsed the same way whichever
//! operation reads it: content that is an X12 interchange is parsed by all of them and
//! its values carried into each one's own answer, and content that is not one is refused
//! by all of them, with the reader's own sentence, in the error each operation's model
//! declares.
//!
//! The rows differ along what the rule generalises over: the transaction set, the X12
//! release, the delimiters the file is written with (X12 fixes none — a reader takes them
//! from the file's own ISA segment), how many transaction sets it holds, and the way an
//! unparsable file fails. `TestParsing`'s own answer — the document's shape, the split
//! options, the validation messages and the read's failure modes — is asserted by
//! `tests/all/test_parsing.rs`; the transformer job's by `tests/all/transformer_jobs.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;

use crate::onboarding_loop::{b2bi_router, call, error_code, given_bucket, s3};

/// The bucket every row of these tables stores its EDI object in.
pub(crate) const BUCKET: &str = "edi-inbound";

/// A valid X12 850 purchase order, written with the classic delimiters: `*` between
/// elements, `>` between components and `~` ending each segment.
///
/// The envelope is a real ISA/GS/ST…SE/GE/IEA interchange: ISA12 `00401` names release
/// 004010, GS08 carries the same version, ST01 the transaction set and BEG03 the purchase
/// order number a mapping reads.
pub(crate) const X12_850: &str = concat!(
    "ISA*00*          *00*          *ZZ*SENDER         *ZZ*RECEIVER       ",
    "*240101*1200*U*00401*000000001*0*P*>~",
    "GS*PO*SENDER*RECEIVER*20240101*1200*1*X*004010~",
    "ST*850*0001~",
    "BEG*00*SA*PO12345**20240101~",
    "N1*ST*Acme Receiving~",
    "N1*BT*Acme Billing~",
    "SE*5*0001~",
    "GE*1*1~",
    "IEA*1*000000001~",
);

/// A valid X12 810 invoice at release 005010, written with `|` between elements and a
/// newline ending each segment — the delimiters are the writer's choice, and a reader
/// takes them from the ISA segment itself.
const X12_810_PIPE: &str = concat!(
    "ISA|00|          |00|          |ZZ|SENDER         |ZZ|RECEIVER       ",
    "|240201|0900|^|00501|000000002|0|P|>\n",
    "GS|IN|SENDER|RECEIVER|20240201|0900|2|X|005010\n",
    "ST|810|0002\n",
    "BIG|20240201|INV-9|20240101|PO12345\n",
    "SE|3|0002\n",
    "GE|1|2\n",
    "IEA|1|000000002\n",
);

/// A valid interchange holding TWO transaction sets in one functional group.
const X12_850_TWO_TRANSACTIONS: &str = concat!(
    "ISA*00*          *00*          *ZZ*SENDER         *ZZ*RECEIVER       ",
    "*240101*1200*U*00401*000000003*0*P*>~",
    "GS*PO*SENDER*RECEIVER*20240101*1200*3*X*004010~",
    "ST*850*0001~",
    "BEG*00*SA*PO-ONE**20240101~",
    "SE*3*0001~",
    "ST*850*0002~",
    "BEG*00*SA*PO-TWO**20240102~",
    "SE*3*0002~",
    "GE*2*3~",
    "IEA*1*000000003~",
);

/// Content that is not an EDI interchange at all — the recorded malformed case.
const NOT_EDI: &str = "this is plainly not an X12 EDI document";

/// An interchange whose transaction set, functional group and interchange are all left
/// open: the file starts like an interchange and is not one.
const X12_850_UNCLOSED: &str = concat!(
    "ISA*00*          *00*          *ZZ*SENDER         *ZZ*RECEIVER       ",
    "*240101*1200*U*00401*000000004*0*P*>~",
    "GS*PO*SENDER*RECEIVER*20240101*1200*4*X*004010~",
    "ST*850*0001~",
    "BEG*00*SA*PO-OPEN**20240101~",
);

/// An interchange header followed by data where its element separator belongs, so the
/// file merely begins with the letters `ISA`.
const ISA_WITHOUT_SEPARATOR: &str = "ISAX00X   nothing here declares a delimiter";

/// Store `body` at `key` in [`BUCKET`], creating the bucket first.
pub(crate) async fn given_object(
    router: &smithy_server_runtime::MultiServiceRouter,
    key: &str,
    body: &str,
) {
    given_bucket(router, BUCKET).await;
    let (status, response) = s3(router, "PUT", &format!("/{BUCKET}/{key}"), body.as_bytes()).await;
    assert_eq!(
        status,
        200,
        "storing s3://{BUCKET}/{key} must succeed, got {status} {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// The `TestParsing` request for the object at `key`.
fn test_parsing_request(key: &str) -> Value {
    json!({
        "inputFile": { "bucketName": BUCKET, "key": key },
        "fileFormat": "JSON",
        "ediType": { "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
    })
}

/// The `CreateStarterMappingTemplate` request deriving a template from the sample at
/// `key`.
fn starter_template_request(key: &str) -> Value {
    json!({
        "mappingType": "JSONATA",
        "templateDetails": {
            "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" }
        },
        "outputSampleLocation": { "bucketName": BUCKET, "key": key }
    })
}

/// The message of a refusal, whichever casing the error shape spells it with: a modelled
/// `ValidationException` member is `Message`, the unmodelled channel's is `message`.
fn message(body: &Value) -> String {
    body["Message"]
        .as_str()
        .or_else(|| body["message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// One row: the case, the object key, its content, and — for content that parses — a
/// business value of the transaction set the reader must carry through, or `None` when
/// the content is not an interchange at all.
type ReaderCase<'a> = (&'a str, &'a str, &'a str, Option<&'a str>);

/// RULE: a file that is an X12 interchange is parsed by every operation that reads one,
/// and its transaction-set values reach each operation's own answer — `TestParsing`
/// carries the value itself, `CreateStarterMappingTemplate` carries it under the
/// `PLACEHOLDER: ` marker of the field the caller must map — while a file that is not an
/// interchange is refused by both with the reader's own sentence.
#[tokio::test]
async fn one_reader_answers_for_every_operation_that_reads_an_edi_document() {
    let router = b2bi_router();

    let cases: Vec<ReaderCase> = vec![
        (
            "an X12 850 with the classic delimiters",
            "shared/850.edi",
            X12_850,
            Some("PO12345"),
        ),
        (
            "an X12 810 at release 005010 with pipe delimiters and newline terminators",
            "shared/810-pipe.edi",
            X12_810_PIPE,
            Some("INV-9"),
        ),
        (
            "two transaction sets in one functional group",
            "shared/850-two.edi",
            X12_850_TWO_TRANSACTIONS,
            Some("PO-ONE"),
        ),
        ("content that is not EDI", "shared/not-edi.txt", NOT_EDI, None),
        (
            "an interchange whose envelopes are never closed",
            "shared/unclosed.edi",
            X12_850_UNCLOSED,
            None,
        ),
        (
            "a file that merely begins with the letters ISA",
            "shared/isa-only.txt",
            ISA_WITHOUT_SEPARATOR,
            None,
        ),
        ("an empty object", "shared/empty.txt", "", None),
    ];

    for (case, key, content, business_value) in cases {
        given_object(&router, key, content).await;

        let (parse_status, parsed) =
            call(&router, "TestParsing", test_parsing_request(key)).await;
        let (template_status, template) =
            call(&router, "CreateStarterMappingTemplate", starter_template_request(key)).await;

        match business_value {
            Some(value) => {
                assert_eq!(
                    parse_status, 200,
                    "case {case:?}: TestParsing must parse the file, got {parse_status} \
                     {parsed}"
                );
                let content = parsed["parsedFileContent"].as_str().unwrap_or_default();
                assert!(
                    content.contains(value),
                    "case {case:?}: the parsed document must carry {value:?}, got {content}"
                );

                assert_eq!(
                    template_status, 200,
                    "case {case:?}: CreateStarterMappingTemplate must derive a template \
                     from the sample, got {template_status} {template}"
                );
                let mapping = template["mappingTemplate"].as_str().unwrap_or_default();
                assert!(
                    mapping.contains(&format!("PLACEHOLDER: {value}")),
                    "case {case:?}: the derived template must mark {value:?} as a \
                     placeholder, got {mapping}"
                );
                // The sample's envelope values are carried through verbatim, NOT marked:
                // the caller does not map the interchange envelope.
                assert!(
                    mapping.contains("SENDER"),
                    "case {case:?}: the derived template must carry the sample's \
                     envelope values, got {mapping}"
                );
            }
            None => {
                assert_eq!(
                    parse_status, 400,
                    "case {case:?}: TestParsing must refuse content that is not an \
                     interchange, got {parse_status} {parsed}"
                );
                assert_eq!(
                    error_code(&parsed),
                    "ValidationException",
                    "case {case:?}: the refusal must be a ValidationException, got {parsed}"
                );
                assert_eq!(
                    template_status, 400,
                    "case {case:?}: CreateStarterMappingTemplate must refuse the same \
                     content, got {template_status} {template}"
                );
                assert_eq!(
                    error_code(&template),
                    "ValidationException",
                    "case {case:?}: the refusal must be a ValidationException, got \
                     {template}"
                );
                // One reader, one sentence: both operations report what it found.
                for (operation, body) in [("TestParsing", &parsed), ("CreateStarterMappingTemplate", &template)] {
                    let reported = message(body);
                    assert!(
                        reported.contains("The EDI file could not be parsed."),
                        "case {case:?}: {operation} must report the reader's own refusal, \
                         got {reported:?}"
                    );
                }
            }
        }
    }
}

/// RULE: a sample that genuinely does not resolve is `ResourceNotFoundException` —
/// the error that operation's model declares for a resource that cannot be found — and is
/// told apart from a sample that resolves and holds no interchange (`ValidationException`,
/// above) and from a request that names no sample at all (which generates the template
/// from `templateDetails`).
#[tokio::test]
async fn a_sample_that_does_not_resolve_is_told_from_one_that_does_not_parse() {
    let router = b2bi_router();
    // The bucket exists, so only the key is missing.
    given_object(&router, "shared/present.edi", X12_850).await;

    let (status, response) = call(
        &router,
        "CreateStarterMappingTemplate",
        starter_template_request("shared/absent.edi"),
    )
    .await;
    assert_eq!(
        status, NOT_FOUND_STATUS,
        "a sample that does not resolve must be refused as not found, got {status} {response}"
    );
    assert_eq!(
        error_code(&response),
        "ResourceNotFoundException",
        "a sample that does not resolve must be a ResourceNotFoundException, got {response}"
    );

    // …and a request naming no sample at all is answered from templateDetails.
    let mut without_sample = starter_template_request("shared/present.edi");
    without_sample
        .as_object_mut()
        .expect("a request object")
        .remove("outputSampleLocation");
    let (status, response) =
        call(&router, "CreateStarterMappingTemplate", without_sample).await;
    assert_eq!(
        status, 200,
        "a request naming no sample must still answer with a template, got {status} \
         {response}"
    );
    let mapping = response["mappingTemplate"].as_str().unwrap_or_default();
    assert!(
        !mapping.contains("PLACEHOLDER: "),
        "a template generated without a sample carries no sample value, got {mapping}"
    );
}
