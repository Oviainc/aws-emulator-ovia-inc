// SPDX-License-Identifier: CC0-1.0

//! The S3 Storage Lens configuration lifecycle, driven end to end over HTTP.
//!
//! The rule under test: a `PutStorageLensConfiguration` stores the submitted
//! `StorageLensConfiguration` document WHOLE under the configuration id its URI
//! names, for the account the request names, so every later
//! `GetStorageLensConfiguration` of that id reports exactly the members that
//! document carried and no member of any document put before it — with
//! `StorageLensArn`, which the model documents as read-only, assigned by the
//! service from the configuration's own identity rather than taken from the
//! request. The configuration's tag set is a separate resource: the tagging
//! operations are what report and replace it, `GetStorageLensConfiguration`
//! reports no tags, and a put that names no `Tags` leaves the tag set alone.
//!
//! Every row below differs along something that rule generalises over: how many
//! members the document carries and how deeply they nest, whether it filters
//! with `Include` or `Exclude`, how many buckets and regions it names, which
//! export destination and encryption it asks for, whether it is enabled,
//! whether it already carries a `StorageLensArn`, the region the request is
//! signed for, the account that owns it, and — for the tag set — how many tags
//! it holds, which characters and lengths their keys and values use, and in
//! which order they were submitted.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router, which deserializes
//! it per the Smithy model, runs the handler and serializes the restXml
//! response. The router, the request builder and the XML escaping are the
//! access point suite's, reused rather than copied.
//!
//! The document readers and writers below (`leaves_of`, `document_xml`,
//! `tags_xml` and friends) are `pub(crate)` because the Storage Lens GROUP
//! suite compares whole documents the same way rather than writing a second
//! XML walker.

use crate::access_point_lifecycle::{
    call_in_region, xml_escape, xml_unescape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION,
    XMLNS,
};

/// A second signing region, used to pin that a configuration's ARN names the
/// region its request was signed for rather than a fixed one.
const OTHER_REGION: &str = "eu-central-1";

/// A third account, so the refusal rows differ in more than one account the
/// caller holds no credentials for.
const THIRD_ACCOUNT: &str = "555555555555";

/// The status every Storage Lens configuration call whose response carries no
/// document answers a success with — the status this whole service answers an
/// empty-body success with, re-exported here because the sibling Storage Lens
/// suites read it from this module.
pub(crate) use crate::access_point_lifecycle::NO_CONTENT;

/// One leaf element of a document: the `/`-separated path of element names from
/// the document's root element (exclusive), and the element's text. An empty
/// value is an element with no text at all (`<BucketLevel/>`), which is how an
/// all-optional container is written.
///
/// `pub(crate)` because the sibling suites in `tests/all/` read a nested answer
/// through [`leaves_of`] rather than each writing a second XML reader.
pub(crate) type Leaf = (String, String);

/// One leaf, spelled for a table row.
pub(crate) fn leaf(path: &str, value: &str) -> Leaf {
    (path.to_string(), value.to_string())
}

/// One tag, spelled for a table row.
pub(crate) fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `key` with its first character changed, which is a different key of the same
/// length: no tag key in the table starts with `z`, and every key is at least
/// one character long (`TagKeyString` is 1..128).
fn relabelled(key: &str) -> String {
    let mut characters: Vec<char> = key.chars().collect();
    if let Some(first) = characters.first_mut() {
        *first = 'z';
    }
    characters.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Reading and writing a document as its leaves
//
// `Reply::texts` scans for one element name anywhere in the body, which cannot
// tell `AccountLevel/ActivityMetrics/IsEnabled` from
// `AccountLevel/BucketLevel/ActivityMetrics/IsEnabled` — and a Storage Lens
// document repeats `IsEnabled` at seven depths. Whole-document equality needs
// the PATH of every leaf, which is what these two functions add.
// ---------------------------------------------------------------------------

/// An element name without its namespace prefix.
fn local_name(raw: &str) -> String {
    match raw.split_once(':') {
        Some((_, local)) => local.to_string(),
        None => raw.to_string(),
    }
}

/// Every leaf element of `body`, as (path, text) in document order, with the
/// path rooted at the document's root element (so the root itself is not in any
/// path).
///
/// The only documents this reads are the ones this service serialises, so it
/// covers exactly what the restXml serializer emits — elements with optional
/// attributes, the empty form, and text carrying the five predefined entities —
/// and PANICS on anything else rather than guessing, so a malformed body fails
/// the test loudly instead of comparing as nothing.
pub(crate) fn leaves_of(body: &str) -> Vec<Leaf> {
    /// name, whether any child element was seen, accumulated text.
    type Open = (String, bool, String);

    fn path_of(stack: &[Open], name: &str) -> String {
        let mut parts: Vec<&str> = stack
            .iter()
            .skip(1)
            .map(|(open, _, _)| open.as_str())
            .collect();
        parts.push(name);
        parts.join("/")
    }

    let mut leaves: Vec<Leaf> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let Some(after) = rest.strip_prefix('<') else {
            let end = rest.find('<').unwrap_or(rest.len());
            if let Some(current) = stack.last_mut() {
                current.2.push_str(&rest[..end]);
            }
            rest = &rest[end..];
            continue;
        };
        let end = after
            .find('>')
            .unwrap_or_else(|| panic!("unterminated element in the answer: {body}"));
        let tag = &after[..end];
        rest = &after[end + 1..];
        // The XML declaration, a comment or a processing instruction.
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        if let Some(closing) = tag.strip_prefix('/') {
            let (name, had_child, text) = stack.pop().expect("an element to close");
            assert_eq!(
                name,
                local_name(closing.trim()),
                "mismatched closing element in the answer: {body}"
            );
            if !had_child {
                let path = path_of(&stack, &name);
                if !path.is_empty() {
                    leaves.push((path, xml_unescape(&text)));
                }
            }
            if let Some(parent) = stack.last_mut() {
                parent.1 = true;
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let opening = tag.trim_end_matches('/');
        let name = local_name(opening.split_whitespace().next().unwrap_or(opening));
        if self_closing {
            let path = path_of(&stack, &name);
            if !path.is_empty() {
                leaves.push((path, String::new()));
            }
            if let Some(parent) = stack.last_mut() {
                parent.1 = true;
            }
        } else {
            stack.push((name, false, String::new()));
        }
    }
    assert!(
        stack.is_empty(),
        "the answer left elements unclosed: {body}"
    );
    leaves
}

/// The leaves of `body`, sorted, so two documents compare as documents rather
/// than as a member order no part of the contract fixes.
pub(crate) fn sorted_leaves(body: &str) -> Vec<Leaf> {
    let mut leaves = leaves_of(body);
    leaves.sort();
    leaves
}

/// `leaves` sorted, for comparison against [`sorted_leaves`].
pub(crate) fn sorted(leaves: &[Leaf]) -> Vec<Leaf> {
    let mut sorted = leaves.to_vec();
    sorted.sort();
    sorted
}

/// The element tree `leaves` describe, serialized.
///
/// Successive leaves sharing a parent path go into the same parent element, so
/// two leaves at `Include/Buckets/Arn` are two `<Arn>` elements in one
/// `<Buckets>`.
pub(crate) fn document_xml(leaves: &[Leaf]) -> String {
    let mut xml = String::new();
    let mut open: Vec<&str> = Vec::new();
    for (path, value) in leaves {
        let segments: Vec<&str> = path.split('/').collect();
        let (name, parents) = segments
            .split_last()
            .expect("a leaf path names at least one element");
        let mut shared = 0;
        while shared < open.len() && shared < parents.len() && open[shared] == parents[shared] {
            shared += 1;
        }
        while open.len() > shared {
            let closing = open.pop().expect("an element to close");
            xml.push_str(&format!("</{closing}>"));
        }
        for parent in &parents[shared..] {
            xml.push_str(&format!("<{parent}>"));
            open.push(parent);
        }
        if value.is_empty() {
            xml.push_str(&format!("<{name}/>"));
        } else {
            xml.push_str(&format!("<{name}>{}</{name}>", xml_escape(value)));
        }
    }
    while let Some(closing) = open.pop() {
        xml.push_str(&format!("</{closing}>"));
    }
    xml
}

/// The `<Tags>` element a request carries, or nothing at all when the request
/// names no tag set.
pub(crate) fn tags_xml(tags: Option<&[(String, String)]>) -> String {
    let Some(tags) = tags else {
        return String::new();
    };
    let mut xml = String::from("<Tags>");
    for (key, value) in tags {
        xml.push_str(&format!(
            "<Tag><Key>{}</Key><Value>{}</Value></Tag>",
            xml_escape(key),
            xml_escape(value)
        ));
    }
    xml.push_str("</Tags>");
    xml
}

/// The tag set a tagging answer reports, in the order the document lists it.
///
/// `Key` and `Value` are both required on `StorageLensTag` and on `S3Tag`, so
/// every tag element carries exactly one of each and a `Key` leaf opens a tag.
/// Matching on the path's last segment is what lets the job tagging suite read
/// an `S3TagSet` (wrapped in `<member>`) with this same reader.
pub(crate) fn reported_tags(body: &str) -> Vec<(String, String)> {
    let mut tags: Vec<(String, String)> = Vec::new();
    for (path, value) in leaves_of(body) {
        if path.ends_with("Key") {
            tags.push((value, String::new()));
        } else if path.ends_with("Value") {
            let open = tags
                .last_mut()
                .expect("a <Value> in a <Tag> that opened with its <Key>");
            open.1 = value;
        }
    }
    tags
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// One Storage Lens configuration document, described as its leaves.
///
/// `pub(crate)` with the callers below because the tagging-delete suite drives
/// the same documents through the same front door rather than writing a second
/// table and a second set of request builders.
pub(crate) struct Document {
    pub(crate) case: &'static str,
    /// The leaves below the document root, WITHOUT `Id`: the id is the one the
    /// URI names, so one row can be put under any id.
    leaves: Vec<Leaf>,
}

impl Document {
    /// This document put under `config_id`: `Id` plus the row's own leaves.
    fn submitted(&self, config_id: &str) -> Vec<Leaf> {
        let mut leaves = vec![leaf("Id", config_id)];
        leaves.extend(self.leaves.iter().cloned());
        leaves
    }

    /// What a read of `config_id` must report: the submitted document, with
    /// `StorageLensArn` the one the service assigns — the member is documented
    /// read-only, so a submitted value is replaced rather than stored.
    pub(crate) fn expected(&self, config_id: &str, region: &str) -> Vec<Leaf> {
        let mut leaves: Vec<Leaf> = self
            .submitted(config_id)
            .into_iter()
            .filter(|(path, _)| path != "StorageLensArn")
            .collect();
        leaves.push(leaf("StorageLensArn", &expected_arn(config_id, region)));
        leaves
    }

    fn request_body(&self, config_id: &str, tags: Option<&[(String, String)]>) -> String {
        format!(
            "<PutStorageLensConfigurationRequest xmlns=\"{XMLNS}\">\
             <StorageLensConfiguration>{}</StorageLensConfiguration>{}\
             </PutStorageLensConfigurationRequest>",
            document_xml(&self.submitted(config_id)),
            tags_xml(tags)
        )
    }
}

/// The ARN a configuration of this id must report, in the region the request
/// was signed for: the `storage-lens/<config-id>` resource in the `s3` ARN
/// namespace under the emulator's canonical account — the format
/// `StorageLensConfiguration$StorageLensArn` documents.
fn expected_arn(config_id: &str, region: &str) -> String {
    format!(
        "arn:aws:s3:{region}:{}:storage-lens/{config_id}",
        ::emulator_server::emulator_account_id()
    )
}

/// The documents every assertion below is run over.
///
/// The enum values and the bounds the rows use are the model's own:
/// `S3BucketDestination$Format` is `CSV | Parquet`,
/// `S3BucketDestination$OutputSchemaVersion` is `V_1`, and
/// `SelectionCriteria$MaxDepth` ranges 1..10 — exercised at both published
/// bounds. A value past a modelled bound is refused before any handler runs, so
/// it is the framework's business, not this rule's.
pub(crate) fn documents() -> Vec<Document> {
    vec![
        Document {
            case: "the smallest document the model allows: a required, empty bucket level",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "disabled, with every account-level and bucket-level metric asked for",
            leaves: vec![
                leaf("AccountLevel/ActivityMetrics/IsEnabled", "true"),
                leaf("AccountLevel/BucketLevel/ActivityMetrics/IsEnabled", "true"),
                leaf(
                    "AccountLevel/BucketLevel/AdvancedCostOptimizationMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/BucketLevel/AdvancedDataProtectionMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/BucketLevel/DetailedStatusCodesMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/BucketLevel/AdvancedPerformanceMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/AdvancedCostOptimizationMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/AdvancedDataProtectionMetrics/IsEnabled",
                    "true",
                ),
                leaf("AccountLevel/DetailedStatusCodesMetrics/IsEnabled", "true"),
                leaf("AccountLevel/AdvancedPerformanceMetrics/IsEnabled", "true"),
                leaf("IsEnabled", "false"),
            ],
        },
        Document {
            case: "prefix-level metrics at the shallowest published depth",
            leaves: vec![
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/SelectionCriteria/Delimiter",
                    "/",
                ),
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/SelectionCriteria/MaxDepth",
                    "1",
                ),
                leaf("IsEnabled", "true"),
                leaf("PrefixDelimiter", "/"),
            ],
        },
        Document {
            case: "prefix-level metrics at the deepest published depth, plus a group level",
            leaves: vec![
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/IsEnabled",
                    "true",
                ),
                leaf(
                    "AccountLevel/BucketLevel/PrefixLevel/StorageMetrics/SelectionCriteria/MaxDepth",
                    "10",
                ),
                leaf(
                    "AccountLevel/StorageLensGroupLevel/SelectionCriteria/Include/Arn",
                    "arn:aws:s3:us-west-2:123456789012:storage-lens-group/dev",
                ),
                leaf(
                    "AccountLevel/StorageLensGroupLevel/SelectionCriteria/Include/Arn",
                    "arn:aws:s3:us-west-2:123456789012:storage-lens-group/finance",
                ),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "scoped to three buckets and two regions with Include",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf("Include/Buckets/Arn", "arn:aws:s3:::reports"),
                leaf("Include/Buckets/Arn", "arn:aws:s3:::finance"),
                leaf("Include/Buckets/Arn", "arn:aws:s3:::logs"),
                leaf("Include/Regions/Region", "us-east-1"),
                leaf("Include/Regions/Region", "eu-west-3"),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "scoped by Exclude instead, with one bucket and three regions",
            leaves: vec![
                leaf("AccountLevel/BucketLevel/ActivityMetrics/IsEnabled", "true"),
                leaf("Exclude/Buckets/Arn", "arn:aws:s3:::scratch"),
                leaf("Exclude/Regions/Region", "ap-southeast-2"),
                leaf("Exclude/Regions/Region", "eu-central-1"),
                leaf("Exclude/Regions/Region", "sa-east-1"),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "CSV export to a bucket with SSE-S3, and CloudWatch metrics",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf("DataExport/S3BucketDestination/Format", "CSV"),
                leaf("DataExport/S3BucketDestination/OutputSchemaVersion", "V_1"),
                leaf("DataExport/S3BucketDestination/AccountId", "123456789012"),
                leaf("DataExport/S3BucketDestination/Arn", "arn:aws:s3:::lens-export"),
                leaf("DataExport/S3BucketDestination/Prefix", "reports/lens"),
                leaf("DataExport/S3BucketDestination/Encryption/SSE-S3", ""),
                leaf("DataExport/CloudWatchMetrics/IsEnabled", "true"),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "Parquet export with SSE-KMS and a table destination",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf("DataExport/S3BucketDestination/Format", "Parquet"),
                leaf("DataExport/S3BucketDestination/OutputSchemaVersion", "V_1"),
                leaf("DataExport/S3BucketDestination/AccountId", "210987654321"),
                leaf("DataExport/S3BucketDestination/Arn", "arn:aws:s3:::lens-export"),
                leaf(
                    "DataExport/S3BucketDestination/Encryption/SSE-KMS/KeyId",
                    "arn:aws:kms:us-west-2:123456789012:key/abcd-1234",
                ),
                leaf("DataExport/StorageLensTableDestination/IsEnabled", "true"),
                leaf("IsEnabled", "true"),
            ],
        },
        Document {
            case: "an expanded-prefixes export, an organization, and a delimiter",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf(
                    "ExpandedPrefixesDataExport/S3BucketDestination/Format",
                    "Parquet",
                ),
                leaf(
                    "ExpandedPrefixesDataExport/S3BucketDestination/OutputSchemaVersion",
                    "V_1",
                ),
                leaf(
                    "ExpandedPrefixesDataExport/S3BucketDestination/AccountId",
                    "123456789012",
                ),
                leaf(
                    "ExpandedPrefixesDataExport/S3BucketDestination/Arn",
                    "arn:aws:s3:::lens-expanded",
                ),
                leaf("IsEnabled", "true"),
                leaf(
                    "AwsOrg/Arn",
                    "arn:aws:organizations::123456789012:organization/o-abc1234567",
                ),
                leaf("PrefixDelimiter", "-"),
            ],
        },
        Document {
            case: "a document that already carries a StorageLensArn of its own",
            leaves: vec![
                leaf("AccountLevel/BucketLevel", ""),
                leaf("IsEnabled", "true"),
                leaf(
                    "StorageLensArn",
                    "arn:aws:s3:us-east-1:999999999999:storage-lens/somewhere-else",
                ),
            ],
        },
    ]
}

/// The configuration ids the rows are put under, one per row.
///
/// They differ in length and in which of the characters the wire can ADDRESS a
/// configuration by they use (the letters, digits, hyphen and underscore of
/// `crate::state::addressable_storage_lens_config_id`), including the
/// one-character minimum and the 64-character maximum the model declares for
/// `ConfigId`. The dot the model's pattern also admits is NOT among them — the
/// envelope suite is where a dotted id is held to the URI rule.
fn config_ids() -> Vec<String> {
    vec![
        "a".to_string(),
        "Dashboard-1".to_string(),
        "reports-daily".to_string(),
        "reports_weekly".to_string(),
        "MixedCase_Id-With_Every-Addressable-Character_0123456789".to_string(),
        "x".repeat(64),
        "lens-export".to_string(),
        "lens-export-parquet".to_string(),
        "expanded-prefixes".to_string(),
        "already-has-an-arn".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// Calling the Storage Lens configuration surface
// ---------------------------------------------------------------------------

pub(crate) async fn put_configuration(
    emulator: &Emulator,
    account_id: &str,
    config_id: &str,
    document: &Document,
    tags: Option<&[(String, String)]>,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v20180820/storagelens/{config_id}"),
            account_id,
            &document.request_body(config_id, tags),
        )
        .await
}

pub(crate) async fn get_configuration(
    emulator: &Emulator,
    account_id: &str,
    config_id: &str,
) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/storagelens/{config_id}"),
            account_id,
            "",
        )
        .await
}

pub(crate) async fn put_tagging(
    emulator: &Emulator,
    account_id: &str,
    config_id: &str,
    tags: &[(String, String)],
) -> Reply {
    let body = format!(
        "<PutStorageLensConfigurationTaggingRequest xmlns=\"{XMLNS}\">{}\
         </PutStorageLensConfigurationTaggingRequest>",
        tags_xml(Some(tags))
    );
    emulator
        .call(
            "PUT",
            &format!("/v20180820/storagelens/{config_id}/tagging"),
            account_id,
            &body,
        )
        .await
}

pub(crate) async fn get_tagging(emulator: &Emulator, account_id: &str, config_id: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v20180820/storagelens/{config_id}/tagging"),
            account_id,
            "",
        )
        .await
}

// ---------------------------------------------------------------------------
// The document
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_put_stores_the_whole_document_and_every_read_reports_it() {
    let emulator = Emulator::start();

    for (document, config_id) in documents().iter().zip(config_ids()) {
        let case = document.case;
        let put = put_configuration(&emulator, ACCOUNT, &config_id, document, None).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected the put to be accepted with {NO_CONTENT}, got {} {}",
            put.status, put.body
        );
        assert!(
            put.body.trim().is_empty(),
            "case {case:?}: the operation models no output, so expected an empty body, got {}",
            put.body
        );

        // Read twice: a read reports the stored document, and reporting it
        // changes nothing for the next reader.
        for attempt in ["first read", "second read"] {
            let got = get_configuration(&emulator, ACCOUNT, &config_id).await;
            assert_eq!(
                got.status, 200,
                "case {case:?} ({attempt}): expected 200, got {} {}",
                got.status, got.body
            );
            assert_eq!(
                sorted_leaves(&got.body),
                sorted(&document.expected(&config_id, REGION)),
                "case {case:?} ({attempt}): the read document is not the document put"
            );
        }

        // The tag set of a configuration put without `Tags` is empty, and the
        // document itself reports no tags.
        let tagging = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            tagging.status, 200,
            "case {case:?}: expected 200 from the tagging read, got {} {}",
            tagging.status, tagging.body
        );
        assert_eq!(
            reported_tags(&tagging.body),
            Vec::<(String, String)>::new(),
            "case {case:?}: a configuration put with no Tags carries no tags"
        );
    }
}

#[tokio::test]
async fn a_put_replaces_the_whole_document_rather_than_merging_it() {
    let emulator = Emulator::start();
    let documents = documents();
    let config_id = "replaced";

    // Every row is put over every other row under ONE id, so each replacement
    // is a different pair of documents: a richer one over a poorer one, a
    // poorer one over a richer one, Include over Exclude, and back.
    for previous in &documents {
        for next in &documents {
            let case = format!("{:?} replaced by {:?}", previous.case, next.case);
            for document in [previous, next] {
                let put = put_configuration(&emulator, ACCOUNT, config_id, document, None).await;
                assert_eq!(
                    put.status, NO_CONTENT,
                    "case {case}: expected the put to be accepted with {NO_CONTENT}, got {} {}",
                    put.status, put.body
                );
            }

            let got = get_configuration(&emulator, ACCOUNT, config_id).await;
            assert_eq!(
                sorted_leaves(&got.body),
                sorted(&next.expected(config_id, REGION)),
                "case {case}: the read document must be the last one put, whole"
            );

            // And nothing the earlier document carried alone survives.
            let surviving: Vec<Leaf> = leaves_of(&got.body)
                .into_iter()
                .filter(|held| {
                    previous.submitted(config_id).contains(held)
                        && !next.submitted(config_id).contains(held)
                })
                .collect();
            assert_eq!(
                surviving,
                Vec::<Leaf>::new(),
                "case {case}: members of the replaced document survived the put"
            );
        }
    }
}

#[tokio::test]
async fn the_assigned_arn_follows_the_configuration_identity_and_the_signing_region() {
    let emulator = Emulator::start();
    let document = &documents()[0];

    // The same id in two regions, and two ids in one region: the ARN follows
    // the configuration's identity and the region the request is signed for,
    // and never the ARN a request submitted.
    for region in [REGION, OTHER_REGION] {
        for config_id in ["dashboard", "dashboard-2"] {
            let case = format!("{config_id} in {region}");
            let put = call_in_region(
                &emulator.router,
                region,
                "PUT",
                &format!("/v20180820/storagelens/{config_id}"),
                ACCOUNT,
                &document.request_body(config_id, None),
            )
            .await;
            assert_eq!(
                put.status, NO_CONTENT,
                "case {case}: expected {NO_CONTENT}, got {} {}",
                put.status, put.body
            );

            let got = call_in_region(
                &emulator.router,
                region,
                "GET",
                &format!("/v20180820/storagelens/{config_id}"),
                ACCOUNT,
                "",
            )
            .await;
            let reported: Vec<String> = leaves_of(&got.body)
                .into_iter()
                .filter(|(path, _)| path == "StorageLensArn")
                .map(|(_, value)| value)
                .collect();
            assert_eq!(
                reported,
                vec![expected_arn(config_id, region)],
                "case {case}: the configuration must report exactly the ARN the service assigns"
            );
        }
    }
}

#[tokio::test]
async fn a_put_whose_document_names_another_id_is_refused_and_stores_nothing() {
    let emulator = Emulator::start();
    let document = &documents()[1];
    let config_id = "kept";

    let stored = put_configuration(&emulator, ACCOUNT, config_id, document, None).await;
    assert_eq!(
        stored.status, NO_CONTENT,
        "expected the first put to be accepted with {NO_CONTENT}, got {} {}",
        stored.status, stored.body
    );
    let before = sorted_leaves(&get_configuration(&emulator, ACCOUNT, config_id).await.body);

    // Each row names a different id in the body than in the URI: a longer one,
    // a shorter one, and one differing only in case.
    for other_id in ["kept-elsewhere", "k", "KEPT"] {
        let case = format!("URI names {config_id:?}, document names {other_id:?}");
        let body = format!(
            "<PutStorageLensConfigurationRequest xmlns=\"{XMLNS}\">\
             <StorageLensConfiguration>{}</StorageLensConfiguration>\
             </PutStorageLensConfigurationRequest>",
            document_xml(&document.submitted(other_id))
        );
        let refused = emulator
            .call(
                "PUT",
                &format!("/v20180820/storagelens/{config_id}"),
                ACCOUNT,
                &body,
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case}: expected 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MALFORMED_XML_CODE,
            "case {case}: expected the document to be refused as one this surface \
             will not take, got {}",
            refused.body
        );

        // Neither configuration moved: the one that was there is unchanged, and
        // the id the body named was not created.
        assert_eq!(
            sorted_leaves(&get_configuration(&emulator, ACCOUNT, config_id).await.body),
            before,
            "case {case}: the refused put changed the stored configuration"
        );
        let other = get_configuration(&emulator, ACCOUNT, other_id).await;
        assert_eq!(
            other.status, 404,
            "case {case}: the refused put created {other_id:?}, answered {} {}",
            other.status, other.body
        );
    }
}

#[tokio::test]
async fn configurations_are_scoped_to_the_account_that_put_them() {
    let emulator = Emulator::start();
    let documents = documents();
    let config_id = "shared-id";

    let mine = &documents[4];
    let put = put_configuration(&emulator, ACCOUNT, config_id, mine, None).await;
    assert_eq!(
        put.status, NO_CONTENT,
        "expected {NO_CONTENT}, got {} {}",
        put.status, put.body
    );
    let got = get_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        sorted_leaves(&got.body),
        sorted(&mine.expected(config_id, REGION)),
        "expected the caller's own configuration to read back as it was put"
    );

    // Every call naming another account is refused ahead of the lookup, so one
    // account's configuration is never reached under another's: not by a put
    // that would replace it, and not by a read of the same id.
    let theirs = &documents[6];
    for (case, reply) in [
        (
            "PutStorageLensConfiguration",
            put_configuration(&emulator, OTHER_ACCOUNT, config_id, theirs, None).await,
        ),
        (
            "GetStorageLensConfiguration",
            get_configuration(&emulator, OTHER_ACCOUNT, config_id).await,
        ),
        (
            "GetStorageLensConfiguration, a third account",
            get_configuration(&emulator, THIRD_ACCOUNT, config_id).await,
        ),
    ] {
        assert_eq!(
            (reply.status, reply.code().as_str()),
            (403, "AccessDenied"),
            "case {case:?}: expected a call for another account to be refused, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The refused put stored nothing: the caller's configuration is unchanged.
    let again = get_configuration(&emulator, ACCOUNT, config_id).await;
    assert_eq!(
        sorted_leaves(&again.body),
        sorted(&mine.expected(config_id, REGION)),
        "expected the refused put to leave the stored configuration alone"
    );
}

// ---------------------------------------------------------------------------
// The tag set
// ---------------------------------------------------------------------------

/// The tag sets the tagging assertions are run over.
///
/// The rows differ in how many tags the set holds (none, one, several, the
/// documented maximum), which characters the keys and values use, how long they
/// are (`TagKeyString` is 1..128, `TagValueString` 0..256 — both exercised at
/// their maximum), and the order the tags are submitted in.
pub(crate) fn tag_sets() -> Vec<(&'static str, Vec<(String, String)>)> {
    let maximum = ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MAX_STORAGE_LENS_TAGS;
    vec![
        ("no tags at all", Vec::new()),
        ("one tag", vec![tag("team", "storage")]),
        (
            "several tags, submitted out of key order",
            vec![
                tag("owner", "platform"),
                tag("cost-centre", "4711"),
                tag("Environment", "production"),
            ],
        ),
        (
            "the same keys submitted in the opposite order",
            vec![
                tag("Environment", "production"),
                tag("cost-centre", "4711"),
                tag("owner", "platform"),
            ],
        ),
        (
            "a key and a value using every character class the pattern allows",
            vec![
                tag("tagged by", "a value with spaces"),
                tag("path/like:key=1+2-3@host", "under_scored.value"),
                tag("Ünïcödé", "wërt"),
            ],
        ),
        (
            "a key and a value at their modelled maximum length",
            vec![tag(&"k".repeat(128), &"v".repeat(256))],
        ),
        ("a tag with an empty value", vec![tag("empty", "")]),
        (
            "the documented maximum number of tags",
            (0..maximum)
                .map(|index| tag(&format!("key-{index:02}"), &format!("value-{index:02}")))
                .collect(),
        ),
    ]
}

#[tokio::test]
async fn the_tag_set_is_a_resource_of_its_own() {
    let emulator = Emulator::start();
    let documents = documents();

    for (index, (case, tags)) in tag_sets().into_iter().enumerate() {
        let config_id = format!("tagged-{index}");
        let first = &documents[index % documents.len()];
        let second = &documents[(index + 1) % documents.len()];

        // A put that names a tag set stores it beside the document.
        let put = put_configuration(&emulator, ACCOUNT, &config_id, first, Some(&tags)).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT}, got {} {}",
            put.status, put.body
        );
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            tags,
            "case {case:?}: the tagging read must report exactly the submitted tag set"
        );

        // The document reports no tags: the two are separate resources.
        let document = get_configuration(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            sorted_leaves(&document.body),
            sorted(&first.expected(&config_id, REGION)),
            "case {case:?}: the configuration document must carry no tags"
        );

        // Replacing the tag set replaces it whole and leaves the document alone.
        // Each replacement key is the submitted key with its first character
        // changed, so the replacement set is a different set of the same size
        // and still inside `TagKeyString`'s modelled length.
        let replacement: Vec<(String, String)> = tags
            .iter()
            .rev()
            .map(|(key, value)| tag(&relabelled(key), value))
            .collect();
        let replaced = put_tagging(&emulator, ACCOUNT, &config_id, &replacement).await;
        assert_eq!(
            replaced.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} from the tagging put, got {} {}",
            replaced.status, replaced.body
        );
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            replacement,
            "case {case:?}: the tagging put must replace the whole tag set"
        );
        assert_eq!(
            sorted_leaves(&get_configuration(&emulator, ACCOUNT, &config_id).await.body),
            sorted(&first.expected(&config_id, REGION)),
            "case {case:?}: tagging must not change the configuration document"
        );

        // And replacing the DOCUMENT without naming a tag set leaves the tag
        // set alone: the put is about the document, not about the tags.
        let put = put_configuration(&emulator, ACCOUNT, &config_id, second, None).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} from the second put, got {} {}",
            put.status, put.body
        );
        assert_eq!(
            sorted_leaves(&get_configuration(&emulator, ACCOUNT, &config_id).await.body),
            sorted(&second.expected(&config_id, REGION)),
            "case {case:?}: the second put must replace the document whole"
        );
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            replacement,
            "case {case:?}: a put naming no Tags must leave the tag set alone"
        );

        // An empty tag set put through the tagging operation clears it.
        let cleared = put_tagging(&emulator, ACCOUNT, &config_id, &[]).await;
        assert_eq!(
            cleared.status, NO_CONTENT,
            "case {case:?}: expected {NO_CONTENT} clearing the tag set, got {} {}",
            cleared.status, cleared.body
        );
        let reported = get_tagging(&emulator, ACCOUNT, &config_id).await;
        assert_eq!(
            reported_tags(&reported.body),
            Vec::<(String, String)>::new(),
            "case {case:?}: an empty tag set clears the tags"
        );
    }
}

#[tokio::test]
async fn a_tag_set_past_the_documented_maximum_is_refused() {
    let emulator = Emulator::start();
    let documents = documents();
    let maximum = ::service_com_amazonaws_s3control_awss3controlservicev20180820::state::MAX_STORAGE_LENS_TAGS;

    // One tag past the documented maximum, and far past it.
    for count in [maximum + 1, maximum * 2] {
        let case = format!("{count} tags");
        let tags: Vec<(String, String)> = (0..count)
            .map(|index| tag(&format!("key-{index:03}"), "value"))
            .collect();

        let refused =
            put_configuration(&emulator, ACCOUNT, "too-many-on-put", &documents[0], Some(&tags))
                .await;
        assert_eq!(
            refused.status, 400,
            "case {case} on PutStorageLensConfiguration: expected 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "TooManyTagsException",
            "case {case} on PutStorageLensConfiguration: unexpected refusal {}",
            refused.body
        );
        let missing = get_configuration(&emulator, ACCOUNT, "too-many-on-put").await;
        assert_eq!(
            missing.status, 404,
            "case {case}: the refused put stored a configuration anyway: {}",
            missing.body
        );

        // The same bound on the tagging operation, over a configuration that is
        // really there.
        let put =
            put_configuration(&emulator, ACCOUNT, "too-many-on-tagging", &documents[0], None).await;
        assert_eq!(
            put.status, NO_CONTENT,
            "case {case}: expected {NO_CONTENT} from the put, got {} {}",
            put.status, put.body
        );
        let refused = put_tagging(&emulator, ACCOUNT, "too-many-on-tagging", &tags).await;
        assert_eq!(
            refused.status, 400,
            "case {case} on PutStorageLensConfigurationTagging: expected 400, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.code(),
            "TooManyTagsException",
            "case {case} on PutStorageLensConfigurationTagging: unexpected refusal {}",
            refused.body
        );
        let reported = get_tagging(&emulator, ACCOUNT, "too-many-on-tagging").await;
        assert_eq!(
            reported_tags(&reported.body),
            Vec::<(String, String)>::new(),
            "case {case}: the refused tagging put stored tags anyway"
        );
    }

    // Exactly the maximum is accepted, so the bound refuses only past it.
    let tags: Vec<(String, String)> = (0..maximum)
        .map(|index| tag(&format!("key-{index:03}"), "value"))
        .collect();
    let accepted =
        put_configuration(&emulator, ACCOUNT, "exactly-the-maximum", &documents[0], Some(&tags))
            .await;
    assert_eq!(
        accepted.status, NO_CONTENT,
        "exactly {maximum} tags must be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let reported = get_tagging(&emulator, ACCOUNT, "exactly-the-maximum").await;
    assert_eq!(
        reported_tags(&reported.body),
        tags,
        "exactly {maximum} tags must round-trip"
    );
}

#[tokio::test]
async fn every_call_against_a_configuration_that_is_not_there_is_not_found() {
    let emulator = Emulator::start();

    // A configuration really is there, so "not found" is about the id asked
    // for rather than about an empty store.
    let put = put_configuration(&emulator, ACCOUNT, "present", &documents()[0], None).await;
    assert_eq!(
        put.status, NO_CONTENT,
        "expected {NO_CONTENT} from the put, got {} {}",
        put.status, put.body
    );

    for config_id in ["absent", "Present", "present-1", &"z".repeat(64)] {
        for (operation, reply) in [
            (
                "GetStorageLensConfiguration",
                get_configuration(&emulator, ACCOUNT, config_id).await,
            ),
            (
                "GetStorageLensConfigurationTagging",
                get_tagging(&emulator, ACCOUNT, config_id).await,
            ),
            (
                "PutStorageLensConfigurationTagging",
                put_tagging(&emulator, ACCOUNT, config_id, &[tag("k", "v")]).await,
            ),
        ] {
            let case = format!("{operation} on {config_id:?}");
            assert_eq!(
                reply.status, 404,
                "case {case}: expected 404, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.code(),
                "NoSuchConfiguration",
                "case {case}: unexpected error code in {}",
                reply.body
            );
        }
    }

    // The configuration that IS there was untouched by any of that.
    let got = get_configuration(&emulator, ACCOUNT, "present").await;
    assert_eq!(
        sorted_leaves(&got.body),
        sorted(&documents()[0].expected("present", REGION)),
        "the stored configuration must survive calls against ids that are not there"
    );
}
