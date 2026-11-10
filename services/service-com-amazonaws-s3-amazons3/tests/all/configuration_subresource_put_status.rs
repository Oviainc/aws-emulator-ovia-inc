// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STATUS a successful write of an id-addressed bucket
//! configuration subresource answers.
//!
//! The rule: a write of one of the four bucket configuration subresources S3 addresses by a
//! configuration id — `PUT /{Bucket}?analytics&id=`, `?metrics&id=`, `?inventory&id=` and
//! `?intelligent-tiering&id=` — answers `204 No Content` with an EMPTY body. The payload IS
//! the configuration and the modelled output shape is `smithy.api#Unit`, which
//! `smithy_query(query="operation",operation="PutBucketInventoryConfiguration")` and its
//! three siblings report with no members, so a successful write has nothing to return; the
//! model's `@http` code for all four says 200, while the same subresource's own `DELETE`
//! carries 204 in the model itself and the recorded service answers these writes 204 too.
//!
//! The status is a property of the subresource FAMILY rather than of one call, so the table
//! below spans all four families and, within each, what the rule generalises over: the form
//! of the id (letters and digits, a dash, a period, an underscore, and an id at the
//! published [`MAX_CONFIGURATION_ID_CHARS`] ceiling), how much of the document is filled in
//! (the members the shape requires, and the same configuration with its optional filter and
//! destination members named), and whether the write CREATES the configuration or REPLACES
//! one the bucket already carries. The recorded calls that filed this are five rows among
//! them.
//!
//! The 204 is asserted NEXT TO the answers it must not swallow: each family's own read,
//! which answers 200 WITH the configuration document (so the write's empty body is not the
//! family's), the family's listing, which answers 200 with a document naming every
//! configuration recorded, and a write S3 refuses, which keeps its own 400 and its own
//! error document.

use service_com_amazonaws_s3_amazons3::ops::put_bucket_analytics_configuration::PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_intelligent_tiering_configuration::PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_inventory_configuration::PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::ops::put_bucket_metrics_configuration::PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS;
use service_com_amazonaws_s3_amazons3::state::{
    MALFORMED_XML_CODE, MAX_CONFIGURATION_ID_CHARS,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_analytics_configuration::{bucket, bucket_arn};
use super::bucket_lifecycle::{router, send, Answer};

/// The XML namespace a client's writer puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The bucket named as an export destination by the documents that carry one. Nothing is
/// stored in it: what the rule is about is the configuration's own round trip.
const DESTINATION_BUCKET: &str = "configuration-put-status-destination";

/// One id-addressed configuration family.
///
/// `subresource` is the query parameter that names it, `success_status` the constant that
/// operation's handler answers a successful write with, and `container` the element both its
/// own read and the family's listing report a configuration in. The four rows ARE the class
/// the rule is stated over.
struct Family {
    subresource: &'static str,
    success_status: u16,
    container: &'static str,
}

/// The four bucket subresources whose write, read and delete are addressed by a
/// configuration id, each with the status its write answers.
fn families() -> Vec<Family> {
    vec![
        Family {
            subresource: "analytics",
            success_status: PUT_BUCKET_ANALYTICS_CONFIGURATION_SUCCESS_STATUS,
            container: "AnalyticsConfiguration",
        },
        Family {
            subresource: "metrics",
            success_status: PUT_BUCKET_METRICS_CONFIGURATION_SUCCESS_STATUS,
            container: "MetricsConfiguration",
        },
        Family {
            subresource: "inventory",
            success_status: PUT_BUCKET_INVENTORY_CONFIGURATION_SUCCESS_STATUS,
            container: "InventoryConfiguration",
        },
        Family {
            subresource: "intelligent-tiering",
            success_status: PUT_BUCKET_INTELLIGENT_TIERING_CONFIGURATION_SUCCESS_STATUS,
            container: "IntelligentTieringConfiguration",
        },
    ]
}

/// How much of a configuration document the request fills in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fill {
    /// Only the members the shape marks `required`.
    Required,
    /// The required members plus the optional ones the shape declares — a filter, and a
    /// destination where the family has one.
    Populated,
}

/// The document a `PUT` of `subresource` carries for `id`, filled in to `fill`.
///
/// Each document is spelled in the member order its own shape declares
/// (`smithy_query(query="shape",shape="AnalyticsConfiguration")` and the `MetricsConfiguration`,
/// `InventoryConfiguration` and `IntelligentTieringConfiguration` siblings), so what travels
/// here is what a client's XML writer sends.
fn document(subresource: &str, id: &str, fill: Fill) -> String {
    let destination = bucket_arn(DESTINATION_BUCKET);
    match subresource {
        "analytics" => {
            let filter = match fill {
                Fill::Required => String::new(),
                Fill::Populated => "<Filter><Prefix>logs/</Prefix></Filter>".to_string(),
            };
            let export = match fill {
                Fill::Required => String::new(),
                Fill::Populated => format!(
                    "<DataExport><OutputSchemaVersion>V_1</OutputSchemaVersion><Destination>\
                     <S3BucketDestination><Format>CSV</Format><Bucket>{destination}</Bucket>\
                     <Prefix>reports/</Prefix></S3BucketDestination></Destination></DataExport>"
                ),
            };
            format!(
                "<AnalyticsConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>{filter}\
                 <StorageClassAnalysis>{export}</StorageClassAnalysis></AnalyticsConfiguration>"
            )
        }
        "metrics" => {
            let filter = match fill {
                Fill::Required => String::new(),
                Fill::Populated => "<Filter><Prefix>reports/</Prefix></Filter>".to_string(),
            };
            format!(
                "<MetricsConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>{filter}\
                 </MetricsConfiguration>"
            )
        }
        "inventory" => {
            let filter = match fill {
                Fill::Required => String::new(),
                Fill::Populated => "<Filter><Prefix>logs/</Prefix></Filter>".to_string(),
            };
            let fields = match fill {
                Fill::Required => String::new(),
                Fill::Populated => {
                    "<OptionalFields><Field>Size</Field><Field>ETag</Field></OptionalFields>"
                        .to_string()
                }
            };
            format!(
                "<InventoryConfiguration xmlns=\"{XMLNS}\"><Destination><S3BucketDestination>\
                 <Bucket>{destination}</Bucket><Format>CSV</Format></S3BucketDestination>\
                 </Destination><IsEnabled>true</IsEnabled>{filter}<Id>{id}</Id>\
                 <IncludedObjectVersions>All</IncludedObjectVersions>{fields}<Schedule>\
                 <Frequency>Daily</Frequency></Schedule></InventoryConfiguration>"
            )
        }
        "intelligent-tiering" => {
            let filter = match fill {
                Fill::Required => String::new(),
                Fill::Populated => "<Filter><Prefix>cold/</Prefix></Filter>".to_string(),
            };
            format!(
                "<IntelligentTieringConfiguration xmlns=\"{XMLNS}\"><Id>{id}</Id>{filter}\
                 <Status>Enabled</Status><Tiering><Days>90</Days>\
                 <AccessTier>ARCHIVE_ACCESS</AccessTier></Tiering>\
                 </IntelligentTieringConfiguration>"
            )
        }
        other => unreachable!("no id-addressed configuration family is named {other:?}"),
    }
}

/// `<METHOD> /<bucket>?<subresource>&id=<id>`.
async fn with_id(
    router: &MultiServiceRouter,
    method: &str,
    bucket: &str,
    subresource: &str,
    id: &str,
    body: &str,
) -> Answer {
    send(
        router,
        method,
        &format!("/{bucket}?{subresource}&id={id}"),
        &[],
        body,
    )
    .await
}

/// One id row: the case it stands for and the id the request states.
///
/// The rows differ along the form of the id — which characters it holds, out of the class
/// every bucket configuration id shares, and how long it is up to the published
/// [`MAX_CONFIGURATION_ID_CHARS`] ceiling. The status under test is the same for all of them,
/// which is the point: it belongs to the write, not to the value addressed.
fn id_rows() -> Vec<(&'static str, String)> {
    vec![
        ("letters and digits", "export7".to_string()),
        ("a dash", "daily-export".to_string()),
        ("an underscore", "daily_export".to_string()),
        ("a period", "daily.export".to_string()),
        ("one character", "x".to_string()),
        (
            "an id at the published ceiling",
            "c".repeat(MAX_CONFIGURATION_ID_CHARS),
        ),
    ]
}

/// A successful write of any of the four id-addressed bucket configuration subresources
/// answers `204 No Content` with an empty body — and the configuration it recorded reads
/// back through the family's own read, which answers 200 WITH a document.
#[tokio::test]
async fn an_id_addressed_configuration_write_answers_204_with_an_empty_body() {
    let router = router();

    // Pinned against the LITERAL here, so the four constants every assertion below reads
    // cannot drift back to the model's `@http` code unnoticed.
    for family in families() {
        assert_eq!(
            family.success_status, 204,
            "case {:?}: a successful id-addressed configuration write answers 204 No Content, \
             got {}",
            family.subresource, family.success_status
        );
    }

    for (index, family) in families().into_iter().enumerate() {
        let name = format!("configuration-put-status-{index}");
        bucket(&router, &name).await;

        for (case, id) in id_rows() {
            // Both fills of the document, in order: the first write CREATES the
            // configuration under this id and the second REPLACES it. The answer is the
            // same 204 with no body for either.
            for (attempt, fill) in [Fill::Required, Fill::Populated].into_iter().enumerate() {
                let case = format!("{} / {case} / {fill:?}", family.subresource);
                let put = with_id(
                    &router,
                    "PUT",
                    &name,
                    family.subresource,
                    &id,
                    &document(family.subresource, &id, fill),
                )
                .await;
                assert_eq!(
                    put.status,
                    family.success_status,
                    "case {case:?} (write {}): expected HTTP {}, got {} with body {:?}",
                    attempt + 1,
                    family.success_status,
                    put.status,
                    String::from_utf8_lossy(&put.body)
                );
                assert!(
                    put.body.is_empty(),
                    "case {case:?} (write {}): a 204 carries no body, got {:?}",
                    attempt + 1,
                    String::from_utf8_lossy(&put.body)
                );

                // The 204 is a successful WRITE, not a no-op: the family's own read reports
                // the configuration back, with a 200 and a document — which is what makes
                // the write's empty body the write's own answer rather than the family's.
                let read = with_id(&router, "GET", &name, family.subresource, &id, "").await;
                assert_eq!(
                    read.status,
                    200,
                    "case {case:?}: expected the read to answer 200, got {} with body {:?}",
                    read.status,
                    String::from_utf8_lossy(&read.body)
                );
                let body = String::from_utf8_lossy(&read.body).into_owned();
                assert!(
                    body.contains(&format!("<{}", family.container)),
                    "case {case:?}: expected the read to report <{}>, got {body:?}",
                    family.container
                );
                assert_eq!(
                    read.element("Id").as_deref(),
                    Some(id.as_str()),
                    "case {case:?}: expected the read to report the id that was put, got {:?}",
                    read.element("Id")
                );
            }
        }

        // The family's LISTING answers 200 with a document naming every configuration the
        // writes recorded: the empty answer belongs to the write alone.
        let listing = send(
            &router,
            "GET",
            &format!("/{name}?{}", family.subresource),
            &[],
            "",
        )
        .await;
        assert_eq!(
            listing.status,
            200,
            "case {:?}: expected the listing to answer 200, got {} with body {:?}",
            family.subresource,
            listing.status,
            String::from_utf8_lossy(&listing.body)
        );
        let mut listed_ids = listing.elements("Id");
        listed_ids.sort();
        let mut written_ids: Vec<String> =
            id_rows().into_iter().map(|(_, id)| id).collect();
        written_ids.sort();
        assert_eq!(
            listed_ids, written_ids,
            "case {:?}: expected the listing to name every id the writes recorded ({:?}), got \
             {listed_ids:?}",
            family.subresource, written_ids
        );
        assert!(
            String::from_utf8_lossy(&listing.body).contains(&format!("<{}", family.container)),
            "case {:?}: expected the listing to report <{}>, got {:?}",
            family.subresource,
            family.container,
            String::from_utf8_lossy(&listing.body)
        );

        // And each family's own DELETE answers the 204 the model declares for it, so the
        // write and the delete of one subresource answer alike.
        let (_, id) = id_rows().into_iter().next_back().expect("an id row");
        let deleted = with_id(&router, "DELETE", &name, family.subresource, &id, "").await;
        assert_eq!(
            deleted.status,
            204,
            "case {:?}: expected the delete to answer the modelled 204, got {} with body {:?}",
            family.subresource,
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
        assert!(
            deleted.body.is_empty(),
            "case {:?}: a 204 carries no body, got {:?}",
            family.subresource,
            String::from_utf8_lossy(&deleted.body)
        );
    }
}

/// The 204 belongs to SUCCESS only: a write of a document that does not describe the
/// configuration the request addresses — its `Id` naming another one — keeps its own refusal
/// status and its own error document, rather than being flattened into a bodyless 204.
#[tokio::test]
async fn a_refused_configuration_write_keeps_its_own_status_and_document() {
    let router = router();
    let name = "configuration-put-status-refused";
    bucket(&router, name).await;

    for family in families() {
        let addressed = "addressed";
        // The document names an id the REQUEST does not, so the payload describes no
        // configuration this call addresses.
        let refused = with_id(
            &router,
            "PUT",
            name,
            family.subresource,
            addressed,
            &document(family.subresource, "another-id", Fill::Required),
        )
        .await;
        assert_eq!(
            refused.status,
            400,
            "case {:?}: expected HTTP 400, got {} with body {:?}",
            family.subresource,
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            !refused.body.is_empty(),
            "case {:?}: a refusal carries its own error document, got an empty body",
            family.subresource
        );
        assert!(
            !refused.error_code().is_empty(),
            "case {:?}: expected a refusal code, got {:?}",
            family.subresource,
            String::from_utf8_lossy(&refused.body)
        );

        // And nothing was recorded under the id the refused write addressed.
        let read = with_id(&router, "GET", name, family.subresource, addressed, "").await;
        assert_ne!(
            read.status, 200,
            "case {:?}: a refused write records nothing, got a read answering 200 with {:?}",
            family.subresource,
            String::from_utf8_lossy(&read.body)
        );
    }

    // The code the four families answer a document that does not validate against the
    // published schema with is one code, whatever the family: `MalformedXML`. Named here so
    // the refusal the rows above pin is the published one rather than any 400.
    assert_eq!(
        MALFORMED_XML_CODE, "MalformedXML",
        "the schema refusal keeps its published code, got {MALFORMED_XML_CODE}"
    );
}
