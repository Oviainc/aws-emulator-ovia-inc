// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetBucketLocation`: the Region a bucket reports, and the
//! envelope-free document it reports it in.
//!
//! Two rules, both from the model:
//!
//! * `GetBucketLocationOutput$LocationConstraint` — "Specifies the Region where the
//!   bucket resides… Buckets in Region us-east-1 have a LocationConstraint of null" — so
//!   the value is the Region the bucket was created in, and absent for the one Region
//!   that is not a `BucketLocationConstraint` value.
//! * The operation carries `aws.customizations#s3UnwrappedXmlOutput` and its output shape
//!   carries `@xmlName("LocationConstraint")`, so the response document is that single
//!   member AS the root element — `<LocationConstraint>us-west-2</LocationConstraint>`,
//!   never a member nested under a root of the same name. Every SDK reads the Region off
//!   the root's own text (botocore's `parse_get_bucket_location` takes `root.text`), so a
//!   nested document reports no Region at all.
//!
//! [`the_unwrapped_output_catalog_matches_the_model`] re-derives the operation list the
//! second rule applies to from the model itself.

use quick_xml::events::Event;
use quick_xml::Reader;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_s3_amazons3::service::wire::UNWRAPPED_XML_OUTPUT_OPERATIONS;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};

/// `GET /<bucket>?location` — `GetBucketLocation`.
async fn get_bucket_location(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?location"), &[], "").await
}

/// The response document's ROOT element name and its own text — what an SDK's
/// `GetBucketLocation` parser reads.
///
/// The text is the root's DIRECT text only: a value nested one element deeper leaves it
/// empty, which is the difference this suite is about.
fn root_element(answer: &Answer) -> (String, String) {
    let body = String::from_utf8_lossy(&answer.body).to_string();
    let mut reader = Reader::from_str(&body);
    let mut name = String::new();
    let mut depth = 0usize;
    let mut text = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                depth += 1;
                if depth == 1 {
                    name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                }
            }
            Event::Empty(empty) => {
                if depth == 0 {
                    name = String::from_utf8_lossy(empty.name().as_ref()).into_owned();
                }
            }
            Event::Text(chunk) if depth == 1 => {
                let decoded = chunk
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"));
                text.push_str(&super::bucket_tagging::unescaped(&decoded));
            }
            Event::End(_) => depth = depth.saturating_sub(1),
            Event::Eof => return (name, text),
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_bucket_reports_the_region_it_was_created_in_as_the_document_root() {
    let router = router();

    // (case, the location constraint the create carried, the Region the location
    // document must report).
    //
    // The rows differ along what the rule generalises over: whether a constraint is
    // present (absent is the only way to ask for `us-east-1`, which is not a
    // `BucketLocationConstraint` value and reports null), the legacy `EU` alias, and
    // Regions in three different prefixes — every constraint below is a value of
    // `BucketLocationConstraint`
    // (`smithy_query(query="shape",shape="BucketLocationConstraint")`).
    let rows: [(&str, Option<&str>, &str); 5] = [
        (
            "no constraint is us-east-1, whose location constraint is null",
            None,
            "",
        ),
        (
            "a named Region is reported as itself",
            Some("us-west-2"),
            "us-west-2",
        ),
        (
            "the legacy EU constraint resides in eu-west-1",
            Some("EU"),
            "eu-west-1",
        ),
        (
            "a Region the issue never cited behaves like any other",
            Some("ap-northeast-3"),
            "ap-northeast-3",
        ),
        (
            "a gov-cloud Region is still just a Region code",
            Some("us-gov-west-1"),
            "us-gov-west-1",
        ),
    ];

    for (index, (case, constraint, expected)) in rows.iter().enumerate() {
        let bucket = format!("location-bucket-{index}");
        let created = create_bucket(&router, &bucket, *constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the create must succeed, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let answer = get_bucket_location(&router, &bucket).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let (root, text) = root_element(&answer);
        assert_eq!(
            root, "LocationConstraint",
            "case {case:?}: the document root must be the output's own @xmlName, got \
             {root:?} in body {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            text, *expected,
            "case {case:?}: expected the root's own text to be {expected:?}, got {text:?} \
             in body {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}

#[tokio::test]
async fn the_location_of_a_bucket_this_emulator_does_not_hold_is_refused() {
    // `GetBucketLocation` models no error, yet S3 answers a bucket it does not hold with
    // `NoSuchBucket` (404) — never an empty location document, which would claim a
    // bucket exists.
    let router = router();
    let answer = get_bucket_location(&router, "location-absent").await;
    assert_eq!(
        answer.status, 404,
        "expected HTTP 404 for a bucket that does not exist, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchBucket",
        "expected the error code NoSuchBucket, got {:?}",
        answer.error_code()
    );
}

#[tokio::test]
async fn an_enveloped_output_keeps_its_envelope() {
    // The unwrapped document is for the operations the model marks, and for nothing
    // else: a bucket's versioning state is still reported inside its own envelope, so
    // the protocol override cannot be flattening every response.
    let router = router();
    let created = create_bucket(&router, "location-enveloped", Some("eu-west-2"), None).await;
    assert_eq!(created.status, 200, "the create must succeed");
    let answer = send(&router, "GET", "/location-enveloped?versioning", &[], "").await;
    assert_eq!(
        answer.status, 200,
        "expected HTTP 200 from GetBucketVersioning, got {} with body {:?}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let (root, _) = root_element(&answer);
    assert_eq!(
        root, "VersioningConfiguration",
        "expected the enveloped root VersioningConfiguration, got {root:?} in body {:?}",
        String::from_utf8_lossy(&answer.body)
    );
}

#[test]
fn the_unwrapped_output_catalog_matches_the_model() {
    // Every operation this service's model marks `aws.customizations#s3UnwrappedXmlOutput`,
    // read from the model itself rather than from a second hand-written list.
    let model_path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-s3-amazons3.json"
    );
    let model: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(model_path).expect("the service's Smithy model must be readable"),
    )
    .expect("the Smithy model must parse as JSON");
    let shapes = model["shapes"]
        .as_object()
        .expect("the model must carry a shapes object");

    let mut marked: Vec<&str> = shapes
        .iter()
        .filter(|(_, shape)| shape["type"] == "operation")
        .filter(|(_, shape)| {
            shape["traits"]
                .get("aws.customizations#s3UnwrappedXmlOutput")
                .is_some()
        })
        .map(|(id, _)| id.split('#').next_back().expect("a shape id names a shape"))
        .collect();
    marked.sort_unstable();

    let mut declared = UNWRAPPED_XML_OUTPUT_OPERATIONS.to_vec();
    declared.sort_unstable();
    assert_eq!(
        declared, marked,
        "UNWRAPPED_XML_OUTPUT_OPERATIONS must name exactly the operations the model marks \
         s3UnwrappedXmlOutput; declared {declared:?}, the model marks {marked:?}"
    );
}
