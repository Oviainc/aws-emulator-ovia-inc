// SPDX-License-Identifier: CC0-1.0

//! How the S3 Storage Lens configuration surface ANSWERS — the status of a
//! success that carries no document, the document a refusal travels in, and the
//! two refusals that are decided from the request's own URI and body — driven
//! end to end over HTTP.
//!
//! Three rules are under test, each over a table rather than one call:
//!
//! * a call of this surface whose response carries no document answers `204 No
//!   Content` with an empty body, and one that reports a document answers 200
//!   with it;
//! * every refusal this surface answers for its own resource — a
//!   configuration that is not there, a document it will not take — travels in
//!   the FLAT document
//!   `<Error><Code/><Message/>[<member/>…]<RequestId/><HostId/></Error>`: one
//!   element, no `<ErrorResponse>` root and no `<Type>`, the pair naming the
//!   request inside it and mirrored in the two correlation headers; a refusal
//!   of a request this service could not PARSE, and a refusal for another
//!   resource of this service, keep the service's own wrapped envelope;
//! * a configuration id the wire cannot address is refused as a URI this
//!   service could not parse, naming the resource path the request addressed;
//!   a document this surface cannot read — or that names another configuration
//!   — is refused as malformed; and a document whose values break a rule the
//!   model documents is refused under that rule's own code — each of them
//!   before anything is stored.
//!
//! Every row differs along something those rules generalise over: the route
//! called and the method it uses, which characters and how many the
//! configuration id carries, which document the request submits and which part
//! of it is wrong, and which class of failure the refusal states.
//!
//! The router, the request builder, the document and tag tables and the XML
//! readers are the access point and Storage Lens configuration suites', reused
//! rather than copied.

use crate::access_point_lifecycle::{Emulator, Reply, ACCOUNT, REGION, XMLNS};
use crate::storage_lens_configuration::{
    document_xml, documents, get_configuration, get_tagging, leaf, put_configuration, put_tagging,
    sorted, sorted_leaves, tag, tags_xml, Leaf, NO_CONTENT,
};
use ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::StorageLensMetrics;

/// The message every refusal of a Storage Lens configuration that is not there
/// states: fixed text, naming no configuration id.
const NO_SUCH_CONFIGURATION: &str = "The specified configuration does not exist.";

/// The refusal a request path this service cannot parse is answered with, as
/// the service states it.
const INVALID_URI_CODE: &str =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::INVALID_URI_CODE;
const INVALID_URI_MESSAGE: &str =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::INVALID_URI_MESSAGE;

/// The refusal a document this surface will not take is answered with, as the
/// service states it.
const MALFORMED_XML_CODE: &str =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MALFORMED_XML_CODE;
const MALFORMED_XML_MESSAGE: &str =
    ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MALFORMED_XML_MESSAGE;

// ---------------------------------------------------------------------------
// Calling the surface
// ---------------------------------------------------------------------------

/// The path a Storage Lens configuration is addressed at.
fn configuration_path(config_id: &str) -> String {
    format!("/v20180820/storagelens/{config_id}")
}

/// The path a Storage Lens configuration's tag set is addressed at.
fn tagging_path(config_id: &str) -> String {
    format!("{}/tagging", configuration_path(config_id))
}

/// The smallest document the model admits, carrying the id the row names: `Id`,
/// `AccountLevel` with its required (and all-optional) `BucketLevel`, and
/// `IsEnabled` — the three members `StorageLensConfiguration` declares required
/// (`smithy_query(query="shape",shape="StorageLensConfiguration")`).
fn smallest_document(config_id: &str) -> Vec<Leaf> {
    vec![
        leaf("Id", config_id),
        leaf("AccountLevel/BucketLevel", ""),
        leaf("IsEnabled", "true"),
    ]
}

/// A `PutStorageLensConfiguration` body carrying `document`.
fn put_body(document: &[Leaf]) -> String {
    format!(
        "<PutStorageLensConfigurationRequest xmlns=\"{XMLNS}\">\
         <StorageLensConfiguration>{}</StorageLensConfiguration>\
         </PutStorageLensConfigurationRequest>",
        document_xml(document)
    )
}

/// A `PutStorageLensConfigurationTagging` body carrying `tags`.
fn put_tagging_body(tags: &[(String, String)]) -> String {
    format!(
        "<PutStorageLensConfigurationTaggingRequest xmlns=\"{XMLNS}\">{}\
         </PutStorageLensConfigurationTaggingRequest>",
        tags_xml(Some(tags))
    )
}

/// Put the smallest document under `config_id`, failing loudly if the setup
/// itself did not take.
async fn configuration(emulator: &Emulator, config_id: &str) {
    let put = emulator
        .call(
            "PUT",
            &configuration_path(config_id),
            ACCOUNT,
            &put_body(&smallest_document(config_id)),
        )
        .await;
    assert_eq!(
        put.status, NO_CONTENT,
        "setting up {config_id:?}: expected {NO_CONTENT} from the put, got {} {}",
        put.status, put.body
    );
}

/// The configuration ids the account holds, read through the list.
async fn listed_ids(emulator: &Emulator) -> Vec<String> {
    emulator
        .call("GET", "/v20180820/storagelens", ACCOUNT, "")
        .await
        .texts("Id")
}

// ---------------------------------------------------------------------------
// Reading an answer as the envelope it travels in
// ---------------------------------------------------------------------------

/// What a failure document says, read off the answer: its error code, its
/// message, and the pair naming the request.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stated {
    code: String,
    message: String,
    request_id: String,
    host_id: String,
}

/// Which document a refusal travels in, as a row names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Envelope {
    /// The flat document the Storage Lens configuration surface answers the
    /// refusals of its own resource in: one `<Error>` element, the correlation
    /// pair inside it.
    Flat,
    /// The envelope the rest of this service answers in:
    /// `<ErrorResponse><Error>…</Error><RequestId/><HostId/></ErrorResponse>`.
    Wrapped,
}

/// Assert that `reply` is a failure in `envelope`, and report what it states.
fn failure(reply: &Reply, case: &str, envelope: Envelope) -> Stated {
    let body = reply.body.as_str();
    match envelope {
        Envelope::Flat => {
            assert!(
                body.starts_with("<Error>") && body.ends_with("</Error>"),
                "case {case:?}: expected the flat <Error> document, got {body}"
            );
            assert!(
                !body.contains("<ErrorResponse"),
                "case {case:?}: expected no <ErrorResponse> wrapper, got {body}"
            );
            assert!(
                !body.contains("<Type>"),
                "case {case:?}: expected no <Type> element in the flat document, got {body}"
            );
        }
        Envelope::Wrapped => {
            assert!(
                body.contains("<ErrorResponse") && body.ends_with("</ErrorResponse>"),
                "case {case:?}: expected the wrapped <ErrorResponse> envelope, got {body}"
            );
        }
    }
    assert_eq!(
        reply.texts("Code").len(),
        1,
        "case {case:?}: expected exactly one <Code> element, got {body}"
    );
    let stated = Stated {
        code: reply.code(),
        message: reply.text("Message").unwrap_or_default(),
        request_id: reply.text("RequestId").unwrap_or_default(),
        host_id: reply.text("HostId").unwrap_or_default(),
    };
    assert!(
        !stated.request_id.is_empty() && !stated.host_id.is_empty(),
        "case {case:?}: expected the document to name the request, got {body}"
    );
    // Where the pair sits is the document's own shape: inside the one element
    // the flat document has, and beside the error element in the wrapped one.
    let error_end = body
        .rfind("</Error>")
        .unwrap_or_else(|| panic!("case {case:?}: expected an <Error> element, got {body}"));
    let pair_at = body
        .find("<RequestId>")
        .unwrap_or_else(|| panic!("case {case:?}: expected a <RequestId> element, got {body}"));
    match envelope {
        Envelope::Flat => assert!(
            pair_at < error_end,
            "case {case:?}: expected the pair inside <Error>, got {body}"
        ),
        Envelope::Wrapped => assert!(
            pair_at > error_end,
            "case {case:?}: expected the pair beside <Error>, got {body}"
        ),
    }
    assert_eq!(
        (
            reply.header("x-amz-request-id"),
            reply.header("x-amz-id-2")
        ),
        (
            Some(stated.request_id.clone()),
            Some(stated.host_id.clone())
        ),
        "case {case:?}: expected the headers to mirror the elements, got {body}"
    );
    stated
}

// ---------------------------------------------------------------------------
// A success that carries no document
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_call_whose_answer_carries_no_document_answers_no_content() {
    let emulator = Emulator::start();
    let documents = documents();

    // The rows differ in the document stored, the tag set put on it and the id
    // it is stored under, so the status is pinned across the surface rather
    // than for one shape of call.
    for (index, document) in documents.iter().enumerate() {
        let case = document.case;
        let config_id = format!("no-content-{index}");
        let tags = vec![tag("row", &index.to_string()), tag(&format!("key-{index}"), "")];

        // Every call of this surface whose answer carries no document, in the
        // order a configuration's life runs: put, tag, untag, delete.
        let put = put_configuration(&emulator, ACCOUNT, &config_id, document, Some(&tags)).await;
        let tagged = put_tagging(&emulator, ACCOUNT, &config_id, &tags).await;
        let untagged = emulator
            .call("DELETE", &tagging_path(&config_id), ACCOUNT, "")
            .await;

        // Before the delete: the two calls that DO report a document answer
        // 200 with it, so the rule is about the answer's content and not about
        // the surface.
        let read = get_configuration(&emulator, ACCOUNT, &config_id).await;
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        let deleted = emulator
            .call("DELETE", &configuration_path(&config_id), ACCOUNT, "")
            .await;

        for (call, reply) in [
            ("PutStorageLensConfiguration", &put),
            ("PutStorageLensConfigurationTagging", &tagged),
            ("DeleteStorageLensConfigurationTagging", &untagged),
            ("DeleteStorageLensConfiguration", &deleted),
        ] {
            assert_eq!(
                reply.status, NO_CONTENT,
                "case {case:?} ({call}): expected {NO_CONTENT}, got {} {}",
                reply.status, reply.body
            );
            assert!(
                reply.body.is_empty(),
                "case {case:?} ({call}): expected no content at all, got {}",
                reply.body
            );
        }

        assert_eq!(
            read.status, 200,
            "case {case:?} (GetStorageLensConfiguration): expected 200, got {} {}",
            read.status, read.body
        );
        assert_eq!(
            sorted_leaves(&read.body),
            sorted(&document.expected(&config_id, REGION)),
            "case {case:?}: the reported document is not the document put"
        );
        assert_eq!(
            reported.status, 200,
            "case {case:?} (GetStorageLensConfigurationTagging): expected 200, got {} {}",
            reported.status, reported.body
        );
    }
}

// ---------------------------------------------------------------------------
// The document a refusal travels in
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_storage_lens_configuration_refusal_travels_in_the_flat_document() {
    let emulator = Emulator::start();
    let absent = "absent-configuration";
    let present = "present-configuration";
    configuration(&emulator, present).await;

    let valid_tags = vec![tag("team", "storage")];
    // One row per class of failure this surface answers for its own resource,
    // across every route that addresses a configuration: five not-found
    // refusals the handlers raise, two for a document the surface will not
    // take, one for a document whose values break a rule the model documents,
    // and one for a tag set past the documented maximum. (A URI this service
    // could not parse is a refusal of the REQUEST rather than of this
    // resource, and is held to the service's own envelope by the URI rows
    // below.)
    let rows: Vec<(&str, &str, String, String, u16, &str, &str)> = vec![
        (
            "GetStorageLensConfiguration on an absent id",
            "GET",
            configuration_path(absent),
            String::new(),
            404,
            "NoSuchConfiguration",
            NO_SUCH_CONFIGURATION,
        ),
        (
            "GetStorageLensConfigurationTagging on an absent id",
            "GET",
            tagging_path(absent),
            String::new(),
            404,
            "NoSuchConfiguration",
            NO_SUCH_CONFIGURATION,
        ),
        (
            "PutStorageLensConfigurationTagging on an absent id",
            "PUT",
            tagging_path(absent),
            put_tagging_body(&valid_tags),
            404,
            "NoSuchConfiguration",
            NO_SUCH_CONFIGURATION,
        ),
        (
            "DeleteStorageLensConfigurationTagging on an absent id",
            "DELETE",
            tagging_path(absent),
            String::new(),
            404,
            "NoSuchConfiguration",
            NO_SUCH_CONFIGURATION,
        ),
        (
            "DeleteStorageLensConfiguration on an absent id",
            "DELETE",
            configuration_path(absent),
            String::new(),
            404,
            "NoSuchConfiguration",
            NO_SUCH_CONFIGURATION,
        ),
        (
            "PutStorageLensConfiguration carrying a document that is not XML",
            "PUT",
            configuration_path(present),
            "not a document at all".to_string(),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "PutStorageLensConfiguration whose document names another id",
            "PUT",
            configuration_path(present),
            put_body(&smallest_document("another-id")),
            400,
            MALFORMED_XML_CODE,
            MALFORMED_XML_MESSAGE,
        ),
        (
            "PutStorageLensConfiguration carrying both scope containers",
            "PUT",
            configuration_path(present),
            put_body(&[
                leaf("Id", present),
                leaf("AccountLevel/BucketLevel", ""),
                leaf("Include/Buckets/Arn", "arn:aws:s3:::reports"),
                leaf("Exclude/Buckets/Arn", "arn:aws:s3:::scratch"),
                leaf("IsEnabled", "true"),
            ]),
            400,
            "InvalidIncludeExcludeContainers",
            "Only one of Include or Exclude should be present.",
        ),
        (
            "PutStorageLensConfigurationTagging carrying more tags than the maximum",
            "PUT",
            tagging_path(present),
            put_tagging_body(
                &(0..=::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MAX_STORAGE_LENS_TAGS)
                    .map(|index| tag(&format!("key-{index:03}"), "value"))
                    .collect::<Vec<(String, String)>>(),
            ),
            400,
            "TooManyTagsException",
            "",
        ),
    ];

    let mut correlations: Vec<String> = Vec::new();
    for (case, method, uri, body, status, code, message) in rows {
        let refused = emulator.call(method, &uri, ACCOUNT, &body).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status}, got {} {}",
            refused.status, refused.body
        );
        let stated = failure(&refused, case, Envelope::Flat);
        assert_eq!(
            stated.code, code,
            "case {case:?}: expected the code {code}, got {}",
            refused.body
        );
        // A refusal whose message is fixed text states exactly that sentence;
        // one that states an instance (how many tags the request carried) only
        // has to say something.
        if message.is_empty() {
            assert!(
                !stated.message.is_empty(),
                "case {case:?}: expected a message beside the code, got {}",
                refused.body
            );
        } else {
            assert_eq!(
                stated.message, message,
                "case {case:?}: expected the fixed message, got {}",
                refused.body
            );
        }
        correlations.push(stated.request_id);
    }

    // Each refusal names its OWN request, so a caller correlating two failures
    // never sees one id for both.
    let distinct: std::collections::BTreeSet<&String> = correlations.iter().collect();
    assert_eq!(
        distinct.len(),
        correlations.len(),
        "expected one correlation id per refusal, got {correlations:?}"
    );
}

#[tokio::test]
async fn a_refusal_for_another_resource_keeps_the_wrapped_envelope() {
    let emulator = Emulator::start();

    // The flat document is this sub-surface's: the rest of this service answers
    // in the wrapped envelope, so a refusal one resource over is unchanged by
    // the rule above. The Storage Lens LISTING route names no configuration at
    // all, which is the boundary of the path space the rule covers.
    for (case, method, uri) in [
        ("an access point that is not there", "GET", "/v20180820/accesspoint/absent-ap"),
        (
            "a continuation token the list never issued",
            "GET",
            "/v20180820/storagelens?nextToken=not-a-token",
        ),
    ] {
        let refused = emulator.call(method, uri, ACCOUNT, "").await;
        assert!(
            refused.status >= 400,
            "case {case:?}: expected a refusal, got {} {}",
            refused.status,
            refused.body
        );
        assert!(
            refused.body.contains("<ErrorResponse"),
            "case {case:?}: expected the wrapped envelope, got {}",
            refused.body
        );
        assert!(
            !refused.code().is_empty(),
            "case {case:?}: expected an error code, got {}",
            refused.body
        );
    }
}

// ---------------------------------------------------------------------------
// A configuration id the wire cannot address
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_configuration_id_the_wire_cannot_address_is_refused_as_an_unparseable_uri() {
    let emulator = Emulator::start();

    // The published bounds of `ConfigId`'s length (1..64) and one row per
    // character class the wire can address a configuration by, against rows
    // carrying a character it cannot — the dot the model's own pattern admits
    // in three positions, and characters outside the pattern altogether.
    let addressable: Vec<(&str, String)> = vec![
        ("one character, the published minimum", "a".to_string()),
        ("sixty-four characters, the published maximum", "b".repeat(64)),
        ("digits only", "0123456789".to_string()),
        ("hyphens and mixed case", "Dashboard-1".to_string()),
        ("underscores", "reports_weekly".to_string()),
        (
            "every addressable character class at once",
            "MixedCase_Id-0123456789".to_string(),
        ),
    ];
    let unaddressable: Vec<(&str, String)> = vec![
        ("a dot between two labels", "reports.daily".to_string()),
        ("a leading dot", ".daily".to_string()),
        ("a trailing dot", "daily.".to_string()),
        ("two dots in a row", "reports..daily".to_string()),
        ("a dot at the published maximum length", format!("{}.", "c".repeat(63))),
        ("a plus", "reports+daily".to_string()),
        ("a colon", "reports:daily".to_string()),
        ("an escaped character", "reports%20daily".to_string()),
        ("a tilde", "reports~daily".to_string()),
    ];

    for (case, config_id) in &addressable {
        configuration(&emulator, config_id).await;
        let read = get_configuration(&emulator, ACCOUNT, config_id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected an addressable id to be served, got {} {}",
            read.status, read.body
        );
    }

    for (case, config_id) in &unaddressable {
        // Every route that addresses a configuration by id is refused the same
        // way, and each names the resource path the request addressed.
        for (route, method, uri, body) in [
            (
                "PutStorageLensConfiguration",
                "PUT",
                configuration_path(config_id),
                put_body(&smallest_document(config_id)),
            ),
            (
                "GetStorageLensConfiguration",
                "GET",
                configuration_path(config_id),
                String::new(),
            ),
            (
                "DeleteStorageLensConfiguration",
                "DELETE",
                configuration_path(config_id),
                String::new(),
            ),
            (
                "PutStorageLensConfigurationTagging",
                "PUT",
                tagging_path(config_id),
                put_tagging_body(&[tag("team", "storage")]),
            ),
            (
                "GetStorageLensConfigurationTagging",
                "GET",
                tagging_path(config_id),
                String::new(),
            ),
            (
                "DeleteStorageLensConfigurationTagging",
                "DELETE",
                tagging_path(config_id),
                String::new(),
            ),
        ] {
            let refused = emulator.call(method, &uri, ACCOUNT, &body).await;
            let case = format!("{case} on {route}");
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400, got {} {}",
                refused.status, refused.body
            );
            let stated = failure(&refused, &case, Envelope::Wrapped);
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                (INVALID_URI_CODE, INVALID_URI_MESSAGE),
                "case {case:?}: expected the unparseable-URI refusal, got {}",
                refused.body
            );
            // The `URI` member carries the resource path the request
            // addressed, which is what the caller has to change.
            let addressed = uri
                .strip_prefix("/v20180820/")
                .expect("every route of this service is under the API version segment");
            assert_eq!(
                refused.text("URI").as_deref(),
                Some(addressed),
                "case {case:?}: expected the refusal to name the resource path addressed, got {}",
                refused.body
            );
        }
    }

    // The refused puts stored nothing: the account holds exactly the
    // configurations the addressable rows put.
    let mut expected: Vec<String> = addressable
        .iter()
        .map(|(_, config_id)| config_id.clone())
        .collect();
    expected.sort();
    assert_eq!(
        listed_ids(&emulator).await,
        expected,
        "expected only the addressable ids to be stored"
    );
}

// ---------------------------------------------------------------------------
// A document this surface cannot read
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_document_the_storage_lens_surface_cannot_take_is_refused_as_malformed() {
    let emulator = Emulator::start();
    let present = "malformed-rows";
    configuration(&emulator, present).await;
    let before = sorted_leaves(&get_configuration(&emulator, ACCOUNT, present).await.body);

    // One row per way a document can fail to be one this surface takes: a body
    // that is no document at all, one that ends mid-document, a required
    // member left out, a value that is not of its member's type, a value
    // outside its member's declared range, and one outside its member's
    // declared length — for the configuration document and for the tag-set
    // document alike.
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "a body that is not a document",
            "PUT",
            configuration_path(present),
            "}{ not a document".to_string(),
        ),
        (
            "a document that ends mid-element",
            "PUT",
            configuration_path(present),
            format!(
                "<PutStorageLensConfigurationRequest xmlns=\"{XMLNS}\"><StorageLensConfiguration>"
            ),
        ),
        (
            "an empty body where a document is required",
            "PUT",
            configuration_path(present),
            String::new(),
        ),
        (
            "a required member left out",
            "PUT",
            configuration_path(present),
            put_body(&[leaf("Id", present), leaf("IsEnabled", "true")]),
        ),
        (
            "a boolean member carrying something else",
            "PUT",
            configuration_path(present),
            put_body(&[
                leaf("Id", present),
                leaf("AccountLevel/BucketLevel", ""),
                leaf("IsEnabled", "perhaps"),
            ]),
        ),
        (
            "an integer member carrying something else",
            "PUT",
            configuration_path(present),
            put_body(&[
                leaf("Id", present),
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/SelectionCriteria/MaxDepth",
                    "deep",
                ),
                leaf("IsEnabled", "true"),
            ]),
        ),
        (
            // `SelectionCriteria$MaxDepth` is declared range 1..10.
            "a value one past its member's declared range",
            "PUT",
            configuration_path(present),
            put_body(&[
                leaf("Id", present),
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/SelectionCriteria/MaxDepth",
                    "11",
                ),
                leaf("IsEnabled", "true"),
            ]),
        ),
        (
            // `StorageLensConfiguration$PrefixDelimiter` is declared length 0..1.
            "a value one past its member's declared length",
            "PUT",
            configuration_path(present),
            put_body(&[
                leaf("Id", present),
                leaf("AccountLevel/BucketLevel", ""),
                leaf("IsEnabled", "true"),
                leaf("PrefixDelimiter", "--"),
            ]),
        ),
        (
            "a tag-set document whose tag leaves out a required member",
            "PUT",
            tagging_path(present),
            format!(
                "<PutStorageLensConfigurationTaggingRequest xmlns=\"{XMLNS}\"><Tags><Tag>\
                 <Key>team</Key></Tag></Tags></PutStorageLensConfigurationTaggingRequest>"
            ),
        ),
        (
            "a tag-set document that is not a document",
            "PUT",
            tagging_path(present),
            "<<<".to_string(),
        ),
    ];

    for (case, method, uri, body) in rows {
        let refused = emulator.call(method, &uri, ACCOUNT, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {}",
            refused.status, refused.body
        );
        let stated = failure(&refused, case, Envelope::Flat);
        assert_eq!(
            (stated.code.as_str(), stated.message.as_str()),
            (MALFORMED_XML_CODE, MALFORMED_XML_MESSAGE),
            "case {case:?}: expected the malformed-document refusal, got {}",
            refused.body
        );
    }

    // Nothing a refused request carried was stored: the configuration that was
    // there reads back exactly as it was put, and its tag set is still empty.
    assert_eq!(
        sorted_leaves(&get_configuration(&emulator, ACCOUNT, present).await.body),
        before,
        "a refused document changed the stored configuration"
    );
    assert_eq!(
        crate::storage_lens_configuration::reported_tags(
            &get_tagging(&emulator, ACCOUNT, present).await.body
        ),
        Vec::<(String, String)>::new(),
        "a refused tag-set document stored tags anyway"
    );
}

// ---------------------------------------------------------------------------
// A document whose values break a rule the model documents
// ---------------------------------------------------------------------------

/// `true` and `false` as a document writes them.
fn flag(enabled: bool) -> &'static str {
    if enabled {
        "true"
    } else {
        "false"
    }
}

/// A document asking for `family` at the levels the row names: `None` leaves
/// the level's container out, `Some(enabled)` submits it with that flag.
fn metrics_document(
    config_id: &str,
    family: StorageLensMetrics,
    at_account: Option<bool>,
    at_bucket: Option<bool>,
) -> Vec<Leaf> {
    let mut leaves = vec![leaf("Id", config_id)];
    if let Some(enabled) = at_account {
        leaves.push(leaf(
            &format!("AccountLevel/{}/IsEnabled", family.member()),
            flag(enabled),
        ));
    }
    match at_bucket {
        // `AccountLevel$BucketLevel` is required, so a row that asks for
        // nothing of the buckets still submits the container.
        None => leaves.push(leaf("AccountLevel/BucketLevel", "")),
        Some(enabled) => leaves.push(leaf(
            &format!("AccountLevel/BucketLevel/{}/IsEnabled", family.member()),
            flag(enabled),
        )),
    }
    leaves.push(leaf("IsEnabled", "true"));
    leaves
}

/// What each metric family's refusal says, written out here rather than built
/// the way the service builds it: the code names the member `AccountLevel` and
/// `BucketLevel` both declare, and the sentence states what has to be enabled
/// where. The rule is the one `AccountLevel` documents for every family at once
/// ("You must enable Storage Lens metrics consistently at both the account
/// level and bucket level, or your request will fail" —
/// `smithy_query(query="doc",shape="AccountLevel")`), and the five members are
/// the ones both containers declare
/// (`smithy_query(query="shape",shape="AccountLevel,BucketLevel")`).
const MISSING_BUCKET_LEVEL: [(StorageLensMetrics, &str, &str); 5] = [
    (
        StorageLensMetrics::Activity,
        "MissingBucketLevelActivityMetrics",
        "Activity metrics must be enabled at bucket level when activity metrics is enabled at \
         account level",
    ),
    (
        StorageLensMetrics::AdvancedCostOptimization,
        "MissingBucketLevelAdvancedCostOptimizationMetrics",
        "Advanced cost optimization metrics must be enabled at bucket level when advanced cost \
         optimization metrics is enabled at account level",
    ),
    (
        StorageLensMetrics::AdvancedDataProtection,
        "MissingBucketLevelAdvancedDataProtectionMetrics",
        "Advanced data protection metrics must be enabled at bucket level when advanced data \
         protection metrics is enabled at account level",
    ),
    (
        StorageLensMetrics::DetailedStatusCodes,
        "MissingBucketLevelDetailedStatusCodesMetrics",
        "Detailed status codes metrics must be enabled at bucket level when detailed status codes \
         metrics is enabled at account level",
    ),
    (
        StorageLensMetrics::AdvancedPerformance,
        "MissingBucketLevelAdvancedPerformanceMetrics",
        "Advanced performance metrics must be enabled at bucket level when advanced performance \
         metrics is enabled at account level",
    ),
];

#[tokio::test]
async fn a_metric_family_enabled_for_the_account_must_be_enabled_for_its_buckets() {
    let emulator = Emulator::start();

    // Every family the rule spans, against every combination of the two levels
    // it is asked for at: both, one, the other, each submitted but disabled,
    // and neither.
    for (family, code, message) in MISSING_BUCKET_LEVEL {
        for (row, at_account, at_bucket, refused) in [
            ("enabled at both levels", Some(true), Some(true), false),
            ("enabled for the account only", Some(true), None, true),
            (
                "enabled for the account, submitted but disabled for the buckets",
                Some(true),
                Some(false),
                true,
            ),
            ("enabled for the buckets only", None, Some(true), false),
            (
                "submitted but disabled for the account, absent for the buckets",
                Some(false),
                None,
                false,
            ),
            ("asked for at neither level", None, None, false),
        ] {
            let case = format!("{} {row}", family.member());
            let config_id = format!("metrics-{}-{}", family.member(), row.len());
            let document = metrics_document(&config_id, family, at_account, at_bucket);
            let reply = emulator
                .call(
                    "PUT",
                    &configuration_path(&config_id),
                    ACCOUNT,
                    &put_body(&document),
                )
                .await;
            if !refused {
                assert_eq!(
                    reply.status, NO_CONTENT,
                    "case {case:?}: expected a consistent document to be stored, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    get_configuration(&emulator, ACCOUNT, &config_id).await.status,
                    200,
                    "case {case:?}: expected the stored configuration to be readable"
                );
                continue;
            }
            assert_eq!(
                reply.status, 400,
                "case {case:?}: expected 400, got {} {}",
                reply.status, reply.body
            );
            let stated = failure(&reply, &case, Envelope::Flat);
            assert_eq!(
                (stated.code.as_str(), stated.message.as_str()),
                (code, message),
                "case {case:?}: expected the refusal naming this family, got {}",
                reply.body
            );
            assert_eq!(
                get_configuration(&emulator, ACCOUNT, &config_id).await.status,
                404,
                "case {case:?}: the refused put stored a configuration anyway"
            );
        }
    }
}

#[tokio::test]
async fn a_document_scoping_itself_both_ways_at_once_is_refused() {
    let emulator = Emulator::start();
    let include_buckets = leaf("Include/Buckets/Arn", "arn:aws:s3:::reports");
    let include_regions = leaf("Include/Regions/Region", "eu-west-3");
    let exclude_buckets = leaf("Exclude/Buckets/Arn", "arn:aws:s3:::scratch");
    let exclude_regions = leaf("Exclude/Regions/Region", "sa-east-1");

    // The rows differ in which scope containers the document carries and in
    // what each one names: one container scopes the configuration one way and
    // is served, and a document carrying both scopes it two ways and is
    // refused.
    for (index, (case, scope, refused)) in [
        ("neither container", vec![], false),
        ("an Include naming buckets", vec![include_buckets.clone()], false),
        ("an Include naming regions", vec![include_regions.clone()], false),
        (
            "an Include naming both",
            vec![include_buckets.clone(), include_regions.clone()],
            false,
        ),
        ("an Exclude naming buckets", vec![exclude_buckets.clone()], false),
        ("an Exclude naming regions", vec![exclude_regions.clone()], false),
        (
            "both, each naming buckets",
            vec![include_buckets.clone(), exclude_buckets.clone()],
            true,
        ),
        (
            "both, one naming regions and one naming buckets",
            vec![include_regions.clone(), exclude_buckets.clone()],
            true,
        ),
        (
            "both, each naming buckets and regions",
            vec![
                include_buckets.clone(),
                include_regions.clone(),
                exclude_buckets.clone(),
                exclude_regions.clone(),
            ],
            true,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let config_id = format!("scope-{index}");
        let mut document = vec![leaf("Id", &config_id), leaf("AccountLevel/BucketLevel", "")];
        document.extend(scope);
        document.push(leaf("IsEnabled", "true"));
        let reply = emulator
            .call(
                "PUT",
                &configuration_path(&config_id),
                ACCOUNT,
                &put_body(&document),
            )
            .await;
        if !refused {
            assert_eq!(
                reply.status, NO_CONTENT,
                "case {case:?}: expected one scope to be stored, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                get_configuration(&emulator, ACCOUNT, &config_id).await.status,
                200,
                "case {case:?}: expected the stored configuration to be readable"
            );
            continue;
        }
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} {}",
            reply.status, reply.body
        );
        let stated = failure(&reply, case, Envelope::Flat);
        assert_eq!(
            (stated.code.as_str(), stated.message.as_str()),
            (
                "InvalidIncludeExcludeContainers",
                "Only one of Include or Exclude should be present."
            ),
            "case {case:?}: expected the two-scopes refusal, got {}",
            reply.body
        );
        assert_eq!(
            get_configuration(&emulator, ACCOUNT, &config_id).await.status,
            404,
            "case {case:?}: the refused put stored a configuration anyway"
        );
    }
}
