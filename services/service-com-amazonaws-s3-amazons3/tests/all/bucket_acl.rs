// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a general purpose bucket's ACCESS CONTROL LIST: `PutBucketAcl`
//! stores it and `GetBucketAcl` reports it.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour — status, body, error code — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutBucketAcl` "sets the permissions on an existing bucket using access control lists
//!   (ACL)" (the operation's own documentation) from the ONE ACL the request names — a
//!   canned ACL in `x-amz-acl`, explicit grants in the `x-amz-grant-*` headers, or an
//!   `AccessControlPolicy` body — and that ACL becomes the bucket's ACL IN FULL: a
//!   replacement, not a merge. The call answers the modelled 200 with an empty body.
//! * `GetBucketAcl` reports exactly those grants, in the order the ACL names them, beside
//!   the owner — and a bucket that has never had an ACL set reports the canned `private`
//!   ACL it was created with (owner `FULL_CONTROL` alone) without one being written.
//! * "You cannot specify access permission using both the body and the request headers"
//!   (`PutBucketAcl`): a request naming its ACL in two of the three ways is refused, and
//!   so is one naming it in none of them. Every refusal leaves the stored ACL untouched.
//! * A grantee outside what the API publishes is refused: a `Group` URI that is not one of
//!   the three predefined groups, a `type=value` pair whose type is not `id`, `uri` or
//!   `emailAddress`, a `Grant` missing its `Grantee` or its `Permission`, and an ACL
//!   carrying more grants than one ACL may hold.
//! * Email Grantee ACLs are gone in the eight Regions `PutBucketAcl`'s documentation names
//!   ("the request will receive an HTTP 405 (Method Not Allowed) error") and unresolvable
//!   anywhere else, because this emulator holds no email address for the one account it
//!   serves.
//! * "If your bucket uses the bucket owner enforced setting for S3 Object Ownership …
//!   Requests to set ACLs or update ACLs fail and return the AccessControlListNotSupported
//!   error code. Requests to read ACLs are still supported" — and they "return the
//!   bucket-owner-full-control ACL".
//! * An ACL call naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404), and one whose `x-amz-expected-bucket-owner` is not the
//!   bucket's owner answers `AccessDenied` (HTTP 403).

use quick_xml::events::Event;
use quick_xml::Reader;
use service_com_amazonaws_s3_amazons3::state::{
    canonical_user_id, ALL_USERS_GROUP_URI, AUTHENTICATED_USERS_GROUP_URI,
    EMAIL_GRANTEE_DISCONTINUED_REGIONS, GRANTEE_BY_EMAIL, GRANTEE_CANONICAL_USER, GRANTEE_GROUP,
    LOG_DELIVERY_GROUP_URI, MAX_ACL_GRANTS, PERMISSION_FULL_CONTROL, PERMISSION_READ,
    PERMISSION_READ_ACP, PERMISSION_WRITE, PERMISSION_WRITE_ACP,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send, Answer};
use super::bucket_ownership_controls::put_ownership_controls;

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The namespace `Grantee$Type` travels in as an XML ATTRIBUTE: the member carries
/// `xmlName` `xsi:type` with `xmlNamespace`
/// `http://www.w3.org/2001/XMLSchema-instance` (`smithy_query(query="shape",
/// shape="Grant")`), which is the `xsi` prefix every S3 client declares on `<Grantee>`.
const XSI: &str = "http://www.w3.org/2001/XMLSchema-instance";

/// Every value the `BucketCannedACL` enum publishes, in the model's own order
/// (`smithy_query(query="shape",shape="BucketCannedACL")` reports
/// `values: [private, public-read, public-read-write, authenticated-read]`).
const BUCKET_CANNED_ACL_VALUES: [&str; 4] = [
    "private",
    "public-read",
    "public-read-write",
    "authenticated-read",
];

/// One grant as `GetBucketAcl` reports it: who, named how, and what they may do.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ReportedGrant {
    /// The `Grantee$Type` attribute value.
    grantee_type: String,
    /// Whichever of `ID`, `URI` or `EmailAddress` the grantee is named by.
    identity: String,
    /// The `Grant$Permission`.
    permission: String,
}

/// The grant giving `permission` to the predefined group named by `uri`, as a reader
/// reports it.
fn group_grant(uri: &str, permission: &str) -> ReportedGrant {
    ReportedGrant {
        grantee_type: GRANTEE_GROUP.to_string(),
        identity: uri.to_string(),
        permission: permission.to_string(),
    }
}

/// The grant giving `permission` to the account whose canonical user id is `id`, as a
/// reader reports it.
fn user_grant(id: &str, permission: &str) -> ReportedGrant {
    ReportedGrant {
        grantee_type: GRANTEE_CANONICAL_USER.to_string(),
        identity: id.to_string(),
        permission: permission.to_string(),
    }
}

/// The ACL every bucket carries until one is set on it: the owner's `FULL_CONTROL` alone,
/// which is the canned `private` ACL.
fn default_acl() -> Vec<ReportedGrant> {
    vec![user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL)]
}

/// One `<Grant>` of an `AccessControlPolicy` payload, spelled the way a client's XML
/// writer spells it.
#[derive(Debug, Clone, Copy)]
struct GrantXml<'a> {
    /// The `xsi:type` attribute of `<Grantee>`.
    grantee_type: &'a str,
    /// The element naming the grantee and its value — `None` for a `<Grantee>` that names
    /// nobody, which the model allows because only `Type` is required.
    named_by: Option<(&'a str, &'a str)>,
    /// The `<Permission>`, or `None` for a `<Grant>` that grants nothing (also allowed by
    /// the model, since `Grant$Permission` is optional).
    permission: Option<&'a str>,
}

impl<'a> GrantXml<'a> {
    /// A grant to the predefined group named by `uri`.
    fn group(uri: &'a str, permission: &'a str) -> Self {
        Self {
            grantee_type: GRANTEE_GROUP,
            named_by: Some(("URI", uri)),
            permission: Some(permission),
        }
    }

    /// A grant to the account whose canonical user id is `id`.
    fn user(id: &'a str, permission: &'a str) -> Self {
        Self {
            grantee_type: GRANTEE_CANONICAL_USER,
            named_by: Some(("ID", id)),
            permission: Some(permission),
        }
    }
}

/// The `AccessControlPolicy` payload naming `owner` (when any) and `grants`.
///
/// `AccessControlPolicy$Grants` carries `xmlName` `AccessControlList` and is NOT
/// `xmlFlattened` (`smithy_query(query="shape",shape="AccessControlPolicy")`), so the
/// grants are `<Grant>` children of an `<AccessControlList>` element — exactly the
/// document S3's own API reference prints.
fn access_control_policy(owner: Option<&str>, grants: &[GrantXml<'_>]) -> String {
    let mut body = format!("<AccessControlPolicy xmlns=\"{XMLNS}\">");
    if let Some(owner) = owner {
        body.push_str(&format!("<Owner><ID>{owner}</ID></Owner>"));
    }
    body.push_str("<AccessControlList>");
    for grant in grants {
        body.push_str(&format!(
            "<Grant><Grantee xmlns:xsi=\"{XSI}\" xsi:type=\"{}\">",
            grant.grantee_type
        ));
        if let Some((element, value)) = grant.named_by {
            body.push_str(&format!("<{element}>{value}</{element}>"));
        }
        body.push_str("</Grantee>");
        if let Some(permission) = grant.permission {
            body.push_str(&format!("<Permission>{permission}</Permission>"));
        }
        body.push_str("</Grant>");
    }
    body.push_str("</AccessControlList></AccessControlPolicy>");
    body
}

/// `PUT /<bucket>?acl` — `PutBucketAcl`, with whatever headers and body the case names.
async fn put_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> Answer {
    send(router, "PUT", &format!("/{bucket}?acl"), headers, body).await
}

/// `GET /<bucket>?acl` — `GetBucketAcl`.
async fn get_acl(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "GET", &format!("/{bucket}?acl"), &[], "").await
}

/// The owner id and the grants a `GetBucketAcl` body reports, in document order.
///
/// Parsed with `quick-xml` (already a dependency of this workspace), the way the
/// ownership-controls suite beside this one reads its payload, rather than by slicing the
/// body — the grantee's type is an XML ATTRIBUTE, so nothing less would see it.
fn reported_acl(answer: &Answer) -> (Option<String>, Vec<ReportedGrant>) {
    let body = String::from_utf8_lossy(&answer.body).into_owned();
    let mut reader = Reader::from_str(&body);

    let mut owner: Option<String> = None;
    let mut grants: Vec<ReportedGrant> = Vec::new();
    let mut path: Vec<String> = Vec::new();
    let mut grantee_type = String::new();
    let mut identity = String::new();
    loop {
        match reader
            .read_event()
            .unwrap_or_else(|error| panic!("the response body must be well-formed XML: {error}"))
        {
            Event::Start(start) => {
                let name = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                if name == "Grantee" {
                    identity.clear();
                    grantee_type = start
                        .attributes()
                        .flatten()
                        .find(|attribute| {
                            String::from_utf8_lossy(attribute.key.as_ref()).ends_with("type")
                        })
                        .map(|attribute| String::from_utf8_lossy(&attribute.value).into_owned())
                        .unwrap_or_default();
                }
                path.push(name);
            }
            Event::Text(text) => {
                let value = text
                    .xml10_content()
                    .unwrap_or_else(|error| panic!("the response body must decode: {error}"))
                    .into_owned();
                let depth = path.len();
                let element = path.last().map(String::as_str).unwrap_or_default();
                let parent = if depth >= 2 { path[depth - 2].as_str() } else { "" };
                match (parent, element) {
                    ("Owner", "ID") => owner = Some(value),
                    ("Grantee", "ID") | ("Grantee", "URI") | ("Grantee", "EmailAddress") => {
                        identity = value
                    }
                    ("Grant", "Permission") => grants.push(ReportedGrant {
                        grantee_type: grantee_type.clone(),
                        identity: identity.clone(),
                        permission: value,
                    }),
                    _ => {}
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    (owner, grants)
}

/// Just the grants of a `GetBucketAcl` answer.
fn reported_grants(answer: &Answer) -> Vec<ReportedGrant> {
    reported_acl(answer).1
}

/// One row of the round-trip table: the case, the request headers, the request body, and
/// the grants the bucket must report afterwards.
type AclRow = (&'static str, Vec<(&'static str, String)>, String, Vec<ReportedGrant>);

/// The table of ACLs every row of which must round-trip unchanged.
///
/// The rows differ along what the store-and-report rule generalises over: WHICH of the
/// three ways the request names its ACL, WHICH canned ACL (every value the enum
/// publishes), WHICH permission and grantee type a grant names, HOW MANY grantees one
/// header carries, and whether a header value is quoted — the forms
/// `PutBucketAcl`'s own documentation prints. The ACL the issue's recorded call sends (a
/// log-delivery grant) is one row of it.
fn round_trip_rows() -> Vec<AclRow> {
    let owner = canonical_user_id();
    let mut rows: Vec<AclRow> = vec![
        (
            "the canned ACL private: owner FULL_CONTROL and nobody else",
            vec![("x-amz-acl", "private".to_string())],
            String::new(),
            default_acl(),
        ),
        (
            "the canned ACL public-read: AllUsers READ beside the owner",
            vec![("x-amz-acl", "public-read".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "the canned ACL public-read-write: AllUsers READ and WRITE",
            vec![("x-amz-acl", "public-read-write".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_WRITE),
            ],
        ),
        (
            "the canned ACL authenticated-read: AuthenticatedUsers READ",
            vec![("x-amz-acl", "authenticated-read".to_string())],
            String::new(),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "one grant header naming the log-delivery group by its URI",
            vec![(
                "x-amz-grant-write",
                format!("uri={LOG_DELIVERY_GROUP_URI}"),
            )],
            String::new(),
            vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)],
        ),
        (
            "two grant headers, the log-delivery pair, applied in the model's member order",
            vec![
                (
                    "x-amz-grant-write",
                    format!("uri=\"{LOG_DELIVERY_GROUP_URI}\""),
                ),
                (
                    "x-amz-grant-read-acp",
                    format!("uri=\"{LOG_DELIVERY_GROUP_URI}\""),
                ),
            ],
            String::new(),
            vec![
                group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_READ_ACP),
                group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE),
            ],
        ),
        (
            "a grant header naming a canonical user by id",
            vec![("x-amz-grant-full-control", format!("id={owner}"))],
            String::new(),
            vec![user_grant(&owner, PERMISSION_FULL_CONTROL)],
        ),
        (
            "one grant header naming TWO grantees, one quoted and one not",
            vec![(
                "x-amz-grant-read",
                format!("uri=\"{ALL_USERS_GROUP_URI}\", uri={AUTHENTICATED_USERS_GROUP_URI}"),
            )],
            String::new(),
            vec![
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
                group_grant(AUTHENTICATED_USERS_GROUP_URI, PERMISSION_READ),
            ],
        ),
        (
            "a grant header whose type is spelled in another case",
            vec![("x-amz-grant-write-acp", format!("URI={ALL_USERS_GROUP_URI}"))],
            String::new(),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_WRITE_ACP)],
        ),
        (
            "an AccessControlPolicy body naming the owner and two grants",
            vec![],
            access_control_policy(
                Some(&owner),
                &[
                    GrantXml::user(&owner, PERMISSION_FULL_CONTROL),
                    GrantXml::group(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE),
                ],
            ),
            vec![
                user_grant(&owner, PERMISSION_FULL_CONTROL),
                group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE),
            ],
        ),
        (
            "an AccessControlPolicy body naming no owner and one grant",
            vec![],
            access_control_policy(None, &[GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ)]),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ)],
        ),
    ];
    // One row per published `Permission` value, each in a body grant to the same group, so
    // the whole catalog is exercised rather than the four the rows above happen to name.
    for permission in [
        PERMISSION_FULL_CONTROL,
        PERMISSION_READ,
        PERMISSION_READ_ACP,
        PERMISSION_WRITE,
        PERMISSION_WRITE_ACP,
    ] {
        rows.push((
            "a body grant naming one published Permission",
            vec![],
            access_control_policy(None, &[GrantXml::group(ALL_USERS_GROUP_URI, permission)]),
            vec![group_grant(ALL_USERS_GROUP_URI, permission)],
        ));
    }
    rows
}

#[tokio::test]
async fn putting_an_acl_stores_it_and_reads_back_through_get() {
    let router = router();
    let owner = canonical_user_id();

    for (index, (case, headers, body, expected)) in round_trip_rows().into_iter().enumerate() {
        let bucket = format!("acl-round-trip-{index}");
        let created = create_bucket(&router, &bucket, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let put = put_acl(&router, &bucket, &borrowed, &body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected the modelled 200 from PutBucketAcl, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert!(
            put.body.is_empty(),
            "case {case:?}: PutBucketAcl's output is Unit, so the body must be empty, got {:?}",
            String::from_utf8_lossy(&put.body)
        );

        let read = get_acl(&router, &bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetBucketAcl on the bucket just \
             configured, got {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let (reported_owner, grants) = reported_acl(&read);
        assert_eq!(
            reported_owner.as_deref(),
            Some(owner.as_str()),
            "case {case:?}: the ACL's owner is the account that created the bucket, got {:?}",
            reported_owner
        );
        assert_eq!(
            grants, expected,
            "case {case:?}: expected the stored grants back unchanged, got {grants:?}"
        );

        // The second put REPLACES: what comes back is the new ACL alone, with nothing
        // merged in from the ACL above.
        let replacement = put_acl(
            &router,
            &bucket,
            &[("x-amz-grant-read", &format!("uri={ALL_USERS_GROUP_URI}"))],
            "",
        )
        .await;
        assert_eq!(
            replacement.status, 200,
            "case {case:?}: the replacing put must succeed, got {} with body {:?}",
            replacement.status,
            String::from_utf8_lossy(&replacement.body)
        );
        let reread = get_acl(&router, &bucket).await;
        assert_eq!(
            reported_grants(&reread),
            vec![group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ)],
            "case {case:?}: the ACL must be replaced, not merged, got {:?}",
            reported_grants(&reread)
        );
    }
}

#[tokio::test]
async fn a_bucket_with_no_acl_reports_the_private_canned_acl() {
    let router = router();

    // Buckets that have never had an ACL set, one of them in a non-default Region, so the
    // answer is about the default ACL rather than about one bucket.
    for (case, bucket, constraint) in [
        ("a bucket created with no location constraint", "acl-default", None),
        (
            "a bucket created in another Region",
            "acl-default-regional",
            Some("eu-central-1"),
        ),
    ] {
        let created = create_bucket(&router, bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {}",
            created.status
        );

        let read = get_acl(&router, bucket).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading the ACL of a never-configured bucket answers 200, got \
             {} with body {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            reported_grants(&read),
            default_acl(),
            "case {case:?}: a bucket with no ACL carries the canned private ACL, got {:?}",
            reported_grants(&read)
        );

        // Reading twice changes nothing: the read reports, it does not configure.
        let again = get_acl(&router, bucket).await;
        assert_eq!(
            reported_grants(&again),
            default_acl(),
            "case {case:?}: a second read must answer the same way, got {:?}",
            reported_grants(&again)
        );

        // And the bucket still takes a put afterwards, so the rows above reported the
        // default rather than a bucket stuck in it.
        let put = put_acl(&router, bucket, &[("x-amz-acl", "public-read")], "").await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: the bucket must accept an ACL, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert_eq!(
            reported_grants(&get_acl(&router, bucket).await),
            vec![
                user_grant(&canonical_user_id(), PERMISSION_FULL_CONTROL),
                group_grant(ALL_USERS_GROUP_URI, PERMISSION_READ),
            ],
            "case {case:?}: the configured bucket must report what it was given"
        );
    }
}

/// One row of the refusal table: the case, the request headers, the request body, and the
/// wire error code the refusal must carry.
type RefusalRow = (&'static str, Vec<(&'static str, String)>, String, &'static str);

#[tokio::test]
async fn a_refused_acl_changes_nothing() {
    let router = router();
    let bucket = "acl-refusals";
    create_bucket(&router, bucket, None, None).await;
    let owner = canonical_user_id();

    // The ACL the bucket holds throughout: every refusal below must leave it exactly as
    // it is.
    let stored = vec![group_grant(LOG_DELIVERY_GROUP_URI, PERMISSION_WRITE)];
    let setup = put_acl(
        &router,
        bucket,
        &[("x-amz-grant-write", &format!("uri={LOG_DELIVERY_GROUP_URI}"))],
        "",
    )
    .await;
    assert_eq!(
        setup.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        setup.status,
        String::from_utf8_lossy(&setup.body)
    );

    let one_grant_body = access_control_policy(
        None,
        &[GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ)],
    );
    let too_many: Vec<GrantXml<'_>> = std::iter::repeat_n(
        GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ),
        MAX_ACL_GRANTS + 1,
    )
    .collect();
    let foreign_owner = "0".repeat(64);

    // (case, headers, body, expected wire error code). One row per way a request can fall
    // outside what the API accepts; each is refused with HTTP 400.
    let rows: Vec<RefusalRow> = vec![
        (
            "a canned ACL and a grant header together",
            vec![
                ("x-amz-acl", "private".to_string()),
                ("x-amz-grant-read", format!("uri={ALL_USERS_GROUP_URI}")),
            ],
            String::new(),
            "InvalidRequest",
        ),
        (
            "a body and a canned ACL together",
            vec![("x-amz-acl", "private".to_string())],
            one_grant_body.clone(),
            "InvalidRequest",
        ),
        (
            "a body and a grant header together",
            vec![("x-amz-grant-read", format!("uri={ALL_USERS_GROUP_URI}"))],
            one_grant_body.clone(),
            "InvalidRequest",
        ),
        (
            "no canned ACL, no grant header and no body",
            vec![],
            String::new(),
            "MissingRequestBodyError",
        ),
        (
            "a group URI that is not one of the predefined groups",
            vec![(
                "x-amz-grant-read",
                "uri=http://acs.amazonaws.com/groups/s3/LogDeliver".to_string(),
            )],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a grant header pair whose type is not id, uri or emailAddress",
            vec![("x-amz-grant-read", "account=123456789012".to_string())],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a grant header pair that is not a pair at all",
            vec![("x-amz-grant-read", ALL_USERS_GROUP_URI.to_string())],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a grant header pair whose value is empty",
            vec![("x-amz-grant-read", "uri=".to_string())],
            String::new(),
            "InvalidArgument",
        ),
        (
            "a body Grant carrying no Permission",
            vec![],
            access_control_policy(
                None,
                &[GrantXml {
                    grantee_type: GRANTEE_GROUP,
                    named_by: Some(("URI", ALL_USERS_GROUP_URI)),
                    permission: None,
                }],
            ),
            "MalformedACLError",
        ),
        (
            "a body Grantee of type Group that names no URI",
            vec![],
            access_control_policy(
                None,
                &[GrantXml {
                    grantee_type: GRANTEE_GROUP,
                    named_by: None,
                    permission: Some(PERMISSION_READ),
                }],
            ),
            "MalformedACLError",
        ),
        (
            "a body Grantee of type CanonicalUser that names a URI instead of an ID",
            vec![],
            access_control_policy(
                None,
                &[GrantXml {
                    grantee_type: GRANTEE_CANONICAL_USER,
                    named_by: Some(("URI", ALL_USERS_GROUP_URI)),
                    permission: Some(PERMISSION_READ),
                }],
            ),
            "MalformedACLError",
        ),
        (
            "one grant past the number of grants an ACL may hold",
            vec![],
            access_control_policy(None, &too_many),
            "MalformedACLError",
        ),
        (
            "a body Owner that is not the bucket's owner",
            vec![],
            access_control_policy(
                Some(&foreign_owner),
                &[GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ)],
            ),
            "InvalidArgument",
        ),
    ];

    for (case, headers, body, code) in rows {
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = put_acl(&router, bucket, &borrowed, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected the request to be refused with HTTP 400, got {} with \
             body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the documented error code, got {:?}",
            refused.error_code()
        );

        let read = get_acl(&router, bucket).await;
        assert_eq!(
            reported_grants(&read),
            stored,
            "case {case:?}: a refused put must leave the stored ACL untouched, got {:?}",
            reported_grants(&read)
        );
    }

    // A canned ACL outside the published value set is refused by the BOUNDARY — `ACL`
    // targets the `BucketCannedACL` enum, so a value outside it never reaches the handler
    // — and the refusal goes out through the framework's own code rather than one of S3's.
    //
    // `log-delivery-write` is among them deliberately: the Amazon S3 User Guide's canned
    // ACL table documents it for buckets, but this service's model does NOT publish it
    // (`smithy_query(query="shape",shape="BucketCannedACL")` reports exactly the four
    // values in `BUCKET_CANNED_ACL_VALUES`), and the model is the contract this emulator
    // serves.
    for canned in [
        "bucket-owner-full-control",
        "log-delivery-write",
        "PRIVATE",
        "public",
    ] {
        let refused = put_acl(&router, bucket, &[("x-amz-acl", canned)], "").await;
        assert_eq!(
            refused.status, 400,
            "case {canned:?}: a canned ACL outside {BUCKET_CANNED_ACL_VALUES:?} must be \
             refused, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            reported_grants(&get_acl(&router, bucket).await),
            stored,
            "case {canned:?}: a refused put must leave the stored ACL untouched"
        );
    }

    // And every published canned value IS accepted, so the rows above were refused for
    // being outside the set rather than for the header being unreadable.
    for canned in BUCKET_CANNED_ACL_VALUES {
        let put = put_acl(&router, bucket, &[("x-amz-acl", canned)], "").await;
        assert_eq!(
            put.status, 200,
            "case {canned:?}: a published canned ACL must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }
    let _ = owner;
}

#[tokio::test]
async fn an_email_grantee_is_refused_by_region() {
    let router = router();

    // (case, the bucket's Region, the status and code the refusal carries). An Email
    // Grantee ACL is gone in the eight Regions the documentation names and unresolvable
    // everywhere else; both bounds of the published list are covered, plus two Regions
    // just outside it.
    let rows: Vec<(&str, &str, u16, &str)> = vec![
        (
            "the first Region the discontinuation names",
            EMAIL_GRANTEE_DISCONTINUED_REGIONS[0],
            405,
            "MethodNotAllowed",
        ),
        (
            "the last Region the discontinuation names",
            EMAIL_GRANTEE_DISCONTINUED_REGIONS[EMAIL_GRANTEE_DISCONTINUED_REGIONS.len() - 1],
            405,
            "MethodNotAllowed",
        ),
        (
            "a Region in the middle of the published list",
            EMAIL_GRANTEE_DISCONTINUED_REGIONS[3],
            405,
            "MethodNotAllowed",
        ),
        (
            "a Region the list does not name",
            "eu-central-1",
            400,
            "UnresolvableGrantByEmailAddress",
        ),
        (
            "another Region the list does not name",
            "ap-south-1",
            400,
            "UnresolvableGrantByEmailAddress",
        ),
    ];

    for (index, (case, region, status, code)) in rows.into_iter().enumerate() {
        let bucket = format!("acl-email-{index}");
        // `us-east-1` is the default Region, named by carrying no location constraint.
        let constraint = (region != "us-east-1").then_some(region);
        let created = create_bucket(&router, &bucket, constraint, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket must be created first, got {} with body {:?}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );

        // The same grantee in both forms the API accepts it in: the header pair and the
        // body's `AmazonCustomerByEmail` grantee.
        for (form, headers, body) in [
            (
                "the x-amz-grant-read header",
                vec![("x-amz-grant-read", "emailAddress=\"grantee@example.com\"")],
                String::new(),
            ),
            (
                "an AccessControlPolicy body",
                vec![],
                access_control_policy(
                    None,
                    &[GrantXml {
                        grantee_type: GRANTEE_BY_EMAIL,
                        named_by: Some(("EmailAddress", "grantee@example.com")),
                        permission: Some(PERMISSION_READ),
                    }],
                ),
            ),
        ] {
            let refused = put_acl(&router, &bucket, &headers, &body).await;
            assert_eq!(
                refused.status, status,
                "case {case:?}/{form}: expected HTTP {status} for an email grantee in \
                 {region}, got {} with body {:?}",
                refused.status,
                String::from_utf8_lossy(&refused.body)
            );
            assert_eq!(
                refused.error_code(),
                code,
                "case {case:?}/{form}: expected the documented error code, got {:?}",
                refused.error_code()
            );
        }

        // The refusal left the bucket's ACL where it was: still the default.
        assert_eq!(
            reported_grants(&get_acl(&router, &bucket).await),
            default_acl(),
            "case {case:?}: a refused email grantee must leave the ACL untouched"
        );
    }
}

#[tokio::test]
async fn a_bucket_whose_object_ownership_disables_acls_refuses_the_put_and_still_reads() {
    let router = router();
    let bucket = "acl-disabled";
    create_bucket(&router, bucket, None, None).await;

    // An ACL that is set BEFORE ACLs are disabled, so the read below reports the
    // bucket-owner-full-control ACL rather than merely whatever was stored.
    let put = put_acl(&router, bucket, &[("x-amz-acl", "public-read")], "").await;
    assert_eq!(
        put.status, 200,
        "the setup put must succeed, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );

    let disabled = put_ownership_controls(&router, bucket, &["BucketOwnerEnforced"]).await;
    assert_eq!(
        disabled.status, 200,
        "the ownership controls must be set, got {} with body {:?}",
        disabled.status,
        String::from_utf8_lossy(&disabled.body)
    );

    // Every way of naming an ACL is refused the same way while ACLs are disabled.
    for (form, headers, body) in [
        (
            "a canned ACL",
            vec![("x-amz-acl", "private")],
            String::new(),
        ),
        (
            "a grant header",
            vec![("x-amz-grant-read", ALL_USERS_GROUP_URI)],
            String::new(),
        ),
        (
            "an AccessControlPolicy body",
            vec![],
            access_control_policy(
                None,
                &[GrantXml::group(ALL_USERS_GROUP_URI, PERMISSION_READ)],
            ),
        ),
    ] {
        let refused = put_acl(&router, bucket, &headers, &body).await;
        assert_eq!(
            refused.status, 400,
            "case {form:?}: a bucket with ACLs disabled refuses a put with HTTP 400, got \
             {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_code(),
            "AccessControlListNotSupported",
            "case {form:?}: expected the documented error code, got {:?}",
            refused.error_code()
        );
    }

    // "Requests to read ACLs are still supported and return the bucket-owner-full-control
    // ACL with the owner being the account that created the bucket."
    let read = get_acl(&router, bucket).await;
    assert_eq!(
        read.status, 200,
        "reading the ACL of a bucket with ACLs disabled still answers 200, got {} with \
         body {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    assert_eq!(
        reported_grants(&read),
        default_acl(),
        "a bucket with ACLs disabled reports the bucket-owner-full-control ACL, got {:?}",
        reported_grants(&read)
    );
}

#[tokio::test]
async fn the_grantee_type_travels_as_the_xml_attribute_the_api_publishes() {
    let router = router();
    let bucket = "acl-grantee-type-wire";
    create_bucket(&router, bucket, None, None).await;

    // The request spells the grantee's `Type` the way every S3 client does: an ATTRIBUTE
    // of `<Grantee>`, in the namespace `Grant$Grantee` declares. Each row is one grantee
    // type the model publishes that can be stored, so the normalization is pinned over
    // the whole catalog rather than over the one the issue recorded.
    let owner = canonical_user_id();
    for (case, grantee_type, element, value) in [
        (
            "a group grantee named by URI",
            GRANTEE_GROUP,
            "URI",
            LOG_DELIVERY_GROUP_URI.to_string(),
        ),
        (
            "a canonical user grantee named by ID",
            GRANTEE_CANONICAL_USER,
            "ID",
            owner.clone(),
        ),
    ] {
        let body = access_control_policy(
            None,
            &[GrantXml {
                grantee_type,
                named_by: Some((element, &value)),
                permission: Some(PERMISSION_READ),
            }],
        );
        assert!(
            body.contains(&format!("xsi:type=\"{grantee_type}\"")),
            "case {case:?}: the request under test must carry the attribute spelling, got \
             {body:?}"
        );

        let put = put_acl(&router, bucket, &[], &body).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: a body spelling Grantee.Type as the published XML attribute \
             must be accepted, got {} with body {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );

        // And the answer spells it the same way, with the prefix declared — a document a
        // namespace-aware SDK parser can read.
        let read = get_acl(&router, bucket).await;
        let answered = String::from_utf8_lossy(&read.body).into_owned();
        assert!(
            answered.contains(&format!("xsi:type=\"{grantee_type}\"")),
            "case {case:?}: the reported Grantee.Type must be an attribute, got {answered:?}"
        );
        assert!(
            answered.contains(&format!("xmlns:xsi=\"{XSI}\"")),
            "case {case:?}: the attribute's prefix must be declared, got {answered:?}"
        );
        assert!(
            !answered.contains("<xsi:type>"),
            "case {case:?}: Grantee.Type must not also be an element, got {answered:?}"
        );
        assert_eq!(
            reported_grants(&read),
            vec![ReportedGrant {
                grantee_type: grantee_type.to_string(),
                identity: value.clone(),
                permission: PERMISSION_READ.to_string(),
            }],
            "case {case:?}: the grant must round-trip through both spellings, got {:?}",
            reported_grants(&read)
        );
    }
}

#[tokio::test]
async fn an_acl_call_on_a_bucket_this_emulator_does_not_hold_is_no_such_bucket() {
    let router = router();
    create_bucket(&router, "held-acl-bucket", None, None).await;

    // Names that are NOT the one bucket created above: a different name, a prefix of it,
    // an extension of it, and one differing only in case (bucket names are
    // case-sensitive). `NoSuchBucket` carries `httpError 404` in the model.
    for bucket in [
        "absent-acl-bucket",
        "held",
        "held-acl-bucket-2",
        "Held-Acl-Bucket",
    ] {
        for (operation, answer) in [
            (
                "PutBucketAcl",
                put_acl(&router, bucket, &[("x-amz-acl", "private")], "").await,
            ),
            ("GetBucketAcl", get_acl(&router, bucket).await),
        ] {
            assert_eq!(
                answer.status, 404,
                "case {bucket:?}/{operation}: expected HTTP 404 for a bucket this emulator \
                 does not hold, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "NoSuchBucket",
                "case {bucket:?}/{operation}: expected the modelled error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the bucket that IS held answers normally, so the rows above failed on the lookup
    // rather than on the operations being unreachable.
    let put = put_acl(&router, "held-acl-bucket", &[("x-amz-acl", "private")], "").await;
    assert_eq!(
        put.status, 200,
        "the held bucket must accept an ACL, got {} with body {:?}",
        put.status,
        String::from_utf8_lossy(&put.body)
    );
}

#[tokio::test]
async fn an_acl_call_naming_another_expected_bucket_owner_is_access_denied() {
    let router = router();
    let bucket = "acl-expected-owner";
    create_bucket(&router, bucket, None, None).await;

    // Account ids that are NOT the one account this emulator serves, including one
    // differing from it in a single digit and the canonical user id (which is not an
    // account id at all).
    for expected in ["123456789012", "000000000001", &canonical_user_id()] {
        for (operation, answer) in [
            (
                "PutBucketAcl",
                put_acl(
                    &router,
                    bucket,
                    &[
                        ("x-amz-acl", "private"),
                        ("x-amz-expected-bucket-owner", expected),
                    ],
                    "",
                )
                .await,
            ),
            (
                "GetBucketAcl",
                send(
                    &router,
                    "GET",
                    &format!("/{bucket}?acl"),
                    &[("x-amz-expected-bucket-owner", expected)],
                    "",
                )
                .await,
            ),
        ] {
            assert_eq!(
                answer.status, 403,
                "case {expected:?}/{operation}: an expected owner that is not the bucket's \
                 owner answers HTTP 403, got {} with body {:?}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer.error_code(),
                "AccessDenied",
                "case {expected:?}/{operation}: expected the documented error code, got {:?}",
                answer.error_code()
            );
        }
    }

    // The ACL was not touched by any of the refused puts, and the account's OWN id is
    // accepted — so the rows above failed on the mismatch rather than on the header being
    // rejected outright.
    assert_eq!(
        reported_grants(&get_acl(&router, bucket).await),
        default_acl(),
        "a refused put must leave the bucket's ACL untouched"
    );
    let allowed = put_acl(
        &router,
        bucket,
        &[
            ("x-amz-acl", "public-read"),
            ("x-amz-expected-bucket-owner", engine::emulator_account_id()),
        ],
        "",
    )
    .await;
    assert_eq!(
        allowed.status, 200,
        "the account's own id must be accepted as the expected bucket owner, got {} with \
         body {:?}",
        allowed.status,
        String::from_utf8_lossy(&allowed.body)
    );
}
