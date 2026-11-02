// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an ANONYMOUS object read: a `GetObject` or `HeadObject` that
//! presents no credential at all — no `Authorization` header and no presigned query — which
//! is how every browser, `curl` and unsigned SDK client fetches public S3 content.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour an unsigned client sees: the status, the error document's `<Code>` and
//! `<Message>`, and the bytes a served read answers with.
//!
//! The rule under test: a request carrying no credential speaks for no principal, so S3
//! serves it only what the bucket grants to EVERYONE and refuses it with HTTP 403
//! `AccessDenied` otherwise — never a routing failure, and never the 404 that would say the
//! object is not there.
//!
//! * "If you grant READ access to the anonymous user, the `GetObject` operation returns the
//!   object without using an authorization header" (`GetObject`'s own documentation), and a
//!   HEAD of that object needs the same permission ("A `HEAD` request has the same options
//!   as a `GET` operation on an object", `HeadObject`'s own documentation).
//! * What the bucket grants to everyone is its public-access configuration as S3 honours it:
//!   `IgnorePublicAcls` makes S3 "ignore all public ACLs on this bucket and any objects that
//!   it contains", and `RestrictPublicBuckets` restricts a bucket with a public policy "to
//!   only AWS service principals and authorized users within the bucket owner's account"
//!   (`PublicAccessBlockConfiguration`'s members) — neither of which an anonymous caller is.
//! * "Requests to a Requester Pays bucket must be authenticated", so an anonymous read of
//!   one is refused however public the object is.
//! * The bucket owner's own signed read is unaffected by any of it, which is what tells an
//!   authorization refusal from the object being unreadable.
//!
//! The rows differ along what the rule generalises over: where the grant comes from (an
//! object ACL, a bucket policy, neither), WHICH grant it is (the canned ACLs, the predefined
//! group it reaches, the action and the resource a policy names, an explicit `Deny`), which
//! Block Public Access switch is in force, who pays for the download, which method asks
//! (GET and HEAD), and which endpoint host the unsigned request is addressed to.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, put_bucket_policy, router, send, Answer};
use super::bucket_ownership_controls::put_ownership_controls;
use super::bucket_public_access_block::put_public_access_block;
use super::object_and_tagging_roundtrip::{get_object, put_object, text};

/// The XML namespace botocore puts on an S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// The bytes every object in this suite holds. An ACL is about who may reach an object, not
/// about what is in it, so one payload serves every row.
const BODY: &[u8] = b"anonymous-subject";

/// The key every row writes.
const KEY: &str = "deals.csv";

/// The code and message S3 answers an authorization refusal with (Amazon S3 "Error
/// responses", list of error codes).
const ACCESS_DENIED: &str = "AccessDenied";
/// See [`ACCESS_DENIED`].
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The message a Requester Pays bucket refuses an unauthenticated caller with — fixed text
/// with no variable parts, since there is no principal to name.
const REQUESTER_PAYS_MESSAGE: &str =
    "Anonymous users cannot invoke requests against Requester Pays buckets. Please \
     authenticate.";

/// Every Block Public Access switch, in the model's own member order
/// (`smithy_query(query="shape",shape="PublicAccessBlockConfiguration")`).
const SETTINGS: [&str; 4] = [
    "BlockPublicAcls",
    "IgnorePublicAcls",
    "BlockPublicPolicy",
    "RestrictPublicBuckets",
];

/// One Block Public Access configuration: the four switches in [`SETTINGS`] order.
type Flags = [bool; 4];

/// Every switch OFF — the configuration a bucket must be put in before a PUBLIC ACL or a
/// PUBLIC policy can be stored on it at all.
const PUBLIC_ACCESS_ALLOWED: Flags = [false, false, false, false];

/// What an unsigned read of one bucket's object must answer.
struct Expected {
    /// The HTTP status.
    status: u16,
    /// The error document's `<Code>`, or `""` for a read that is served.
    code: &'static str,
    /// The error document's `<Message>`, or `""` for a read that is served.
    message: &'static str,
}

/// A read that is served with the object's bytes.
const SERVED: Expected = Expected {
    status: 200,
    code: "",
    message: "",
};

/// A read refused because nothing grants it to everyone.
const REFUSED: Expected = Expected {
    status: 403,
    code: ACCESS_DENIED,
    message: ACCESS_DENIED_MESSAGE,
};

/// One row: a bucket configured a particular way, and what an unsigned read of its object
/// must then answer.
struct Row {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The bucket this row configures — its own, so one row's grants cannot admit another's
    /// read.
    name: &'static str,
    /// The `ObjectOwnership` the bucket is switched to, or `None` to leave the create's own
    /// `BucketOwnerEnforced` default (under which ACLs do not affect permissions).
    ownership: Option<&'static str>,
    /// The Block Public Access configuration to put, or `None` to leave the creation
    /// default (all four switches on).
    block: Option<Flags>,
    /// A canned ACL to put on the object with `PutObjectAcl`, when the row names one.
    object_acl: Option<&'static str>,
    /// A bucket policy to store, when the row names one.
    policy: Option<String>,
    /// The `Payer` to put on the bucket, when the row names one.
    payer: Option<&'static str>,
    /// Whether the ACL put above must be ACCEPTED: a public ACL is refused while
    /// `BlockPublicAcls` is on, and that refusal is itself part of the row's setup.
    acl_accepted: bool,
    /// Whether the bucket owner's own SIGNED read is refused too. A bucket policy is a
    /// resource-based policy in force on every caller, so an explicit `Deny` reaching every
    /// principal reaches the owner as well — every other row's refusal is about the caller
    /// presenting no credential and leaves the owner's read served.
    signed_read_refused: bool,
    /// What the unsigned read must answer.
    expected: Expected,
}

/// The row's defaults: a bucket nothing has configured, holding one object nobody has
/// granted anything on.
impl Default for Row {
    fn default() -> Self {
        Self {
            case: "",
            name: "",
            ownership: None,
            block: None,
            object_acl: None,
            policy: None,
            payer: None,
            acl_accepted: true,
            signed_read_refused: false,
            expected: REFUSED,
        }
    }
}

/// A policy document carrying `statements`, in the form a client sends one: the `Version`
/// the published examples carry, and the `Statement` list.
fn policy_document(statements: &str) -> String {
    format!("{{\"Version\":\"2012-10-17\",\"Statement\":[{statements}]}}")
}

/// One statement allowing `action` on `resource` to the principal spelled `principal`.
fn statement(effect: &str, principal: &str, action: &str, resource: &str) -> String {
    format!(
        "{{\"Effect\":\"{effect}\",\"Principal\":{principal},\"Action\":\"{action}\",\
         \"Resource\":\"{resource}\"}}"
    )
}

/// The ARN of `key` in `bucket`, as a policy names an object: an S3 bucket's ARN carries
/// neither Region nor account, because a bucket name is globally unique.
fn object_arn(bucket: &str, key: &str) -> String {
    format!("arn:aws:s3:::{bucket}/{key}")
}

/// `PUT /<bucket>/<key>?acl` — `PutObjectAcl` with a canned ACL.
async fn put_object_acl(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    canned: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}?acl"),
        &[("x-amz-acl", canned)],
        "",
    )
    .await
}

/// `PUT /<bucket>?requestPayment` — `PutBucketRequestPayment`.
async fn put_request_payment(
    router: &MultiServiceRouter,
    bucket: &str,
    payer: &str,
) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}?requestPayment"),
        &[],
        &format!(
            "<RequestPaymentConfiguration xmlns=\"{XMLNS}\"><Payer>{payer}</Payer>\
             </RequestPaymentConfiguration>"
        ),
    )
    .await
}

/// The `PublicAccessBlockConfiguration` members naming every switch in [`SETTINGS`] order.
fn members(flags: Flags) -> Vec<(&'static str, bool)> {
    SETTINGS
        .iter()
        .zip(flags)
        .map(|(setting, value)| (*setting, value))
        .collect()
}

/// Dispatch one request carrying NO credential: no `Authorization` header and no presigned
/// query parameter, which is what botocore's unsigned signature version puts on the wire.
async fn send_anonymous(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
) -> Answer {
    send_anonymous_with(router, method, host, uri, &[]).await
}

/// [`send_anonymous`], with `headers` on the request — the acknowledgement header a client
/// puts on a Requester Pays read travels there.
async fn send_anonymous_with(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::new())
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// The endpoint hosts an unsigned client reaches this emulator at. None of them names a
/// service in a credential scope, and the first two name none in the host either — a local
/// endpoint URL and its address — so the request routes on its PATH alone.
const ANONYMOUS_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "s3.us-east-1.amazonaws.com"];

/// Stand up `row`'s bucket holding one object, and answer the bucket's name.
async fn configured_bucket(router: &MultiServiceRouter, row: &Row) -> &'static str {
    let name = row.name;
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "case {:?}: the bucket must be created first, got {} with body {:?}",
        row.case,
        created.status,
        text(&created)
    );
    if let Some(ownership) = row.ownership {
        let switched = put_ownership_controls(router, name, &[ownership]).await;
        assert_eq!(
            switched.status, 200,
            "case {:?}: the bucket must take the {ownership:?} ownership setting, got {} \
             with body {:?}",
            row.case,
            switched.status,
            text(&switched)
        );
    }
    // A PUBLIC ACL or policy cannot be stored at all while the creation default blocks it,
    // so every row that stores one lifts the guardrail first and then puts back the
    // configuration the row is about.
    if row.object_acl.is_some() || row.policy.is_some() {
        let lifted = put_public_access_block(router, name, &members(PUBLIC_ACCESS_ALLOWED), &[])
            .await;
        assert_eq!(
            lifted.status, 200,
            "case {:?}: Block Public Access must be liftable, got {} with body {:?}",
            row.case,
            lifted.status,
            text(&lifted)
        );
    }
    let stored = put_object(router, name, KEY, BODY, &[]).await;
    assert_eq!(
        stored.status, 200,
        "case {:?}: the object must be written first, got {} with body {:?}",
        row.case,
        stored.status,
        text(&stored)
    );
    if let Some(policy) = row.policy.as_deref() {
        let put = put_bucket_policy(router, name, policy).await;
        assert_eq!(
            put.status, 204,
            "case {:?}: the bucket policy must be stored, got {} with body {:?}",
            row.case,
            put.status,
            text(&put)
        );
    }
    // The Block Public Access configuration the row is about goes on BEFORE the ACL, so a
    // row whose point is that a public ACL is refused by `BlockPublicAcls` really is refused
    // by it.
    if let Some(flags) = row.block {
        let put = put_public_access_block(router, name, &members(flags), &[]).await;
        assert_eq!(
            put.status, 200,
            "case {:?}: the Block Public Access configuration must be stored, got {} with \
             body {:?}",
            row.case,
            put.status,
            text(&put)
        );
    }
    if let Some(canned) = row.object_acl {
        let put = put_object_acl(router, name, KEY, canned).await;
        let accepted = put.status == 200;
        assert_eq!(
            accepted, row.acl_accepted,
            "case {:?}: the {canned:?} ACL put must {}, got {} with body {:?}",
            row.case,
            if row.acl_accepted {
                "be accepted"
            } else {
                "be refused"
            },
            put.status,
            text(&put)
        );
    }
    if let Some(payer) = row.payer {
        let put = put_request_payment(router, name, payer).await;
        assert_eq!(
            put.status, 200,
            "case {:?}: the request payment configuration must be stored, got {} with body \
             {:?}",
            row.case,
            put.status,
            text(&put)
        );
    }
    name
}

/// The table: every row is one bucket configuration and the answer an unsigned read of its
/// object must get.
///
/// Every policy below is built from the row's own bucket name, so each document names the
/// bucket it is stored on.
fn rows() -> Vec<Row> {
    let everyone = "\"*\"";
    let everyone_as_map = "{\"AWS\":\"*\"}";
    vec![
        Row {
            case: "a bucket nothing has configured, holding a private object",
            name: "anon-default",
            ..Row::default()
        },
        Row {
            case: "an object whose public-read ACL S3 ignores (IgnorePublicAcls on)",
            name: "anon-acl-ignored",
            ownership: Some("ObjectWriter"),
            block: Some([false, true, true, true]),
            object_acl: Some("public-read"),
            ..Row::default()
        },
        Row {
            case: "an object whose public-read ACL S3 honours",
            name: "anon-acl-honoured",
            ownership: Some("ObjectWriter"),
            object_acl: Some("public-read"),
            expected: SERVED,
            ..Row::default()
        },
        Row {
            case: "an object whose public-read-write ACL S3 honours",
            name: "anon-acl-read-write",
            ownership: Some("ObjectWriter"),
            object_acl: Some("public-read-write"),
            expected: SERVED,
            ..Row::default()
        },
        Row {
            case: "an object granted to AuthenticatedUsers, which is not the anonymous caller",
            name: "anon-acl-authenticated",
            ownership: Some("ObjectWriter"),
            object_acl: Some("authenticated-read"),
            ..Row::default()
        },
        Row {
            case: "an object whose public-read ACL BlockPublicAcls refused outright",
            name: "anon-acl-blocked",
            ownership: Some("ObjectWriter"),
            block: Some([true, false, true, true]),
            object_acl: Some("public-read"),
            acl_accepted: false,
            ..Row::default()
        },
        Row {
            case: "a policy allowing the read of THIS key to every principal",
            name: "anon-policy-key",
            policy: Some(policy_document(&statement(
                "Allow",
                everyone,
                "s3:GetObject",
                &object_arn("anon-policy-key", KEY),
            ))),
            expected: SERVED,
            ..Row::default()
        },
        Row {
            case: "a policy allowing every s3 action on every key to every principal",
            name: "anon-policy-wildcard",
            policy: Some(policy_document(&statement(
                "Allow",
                everyone_as_map,
                "s3:*",
                &object_arn("anon-policy-wildcard", "*"),
            ))),
            expected: SERVED,
            ..Row::default()
        },
        Row {
            case: "a policy whose wildcards reach the key through one character each",
            name: "anon-policy-single",
            policy: Some(policy_document(&statement(
                "Allow",
                everyone,
                "s3:GetObjec?",
                &object_arn("anon-policy-single", "deal?.csv"),
            ))),
            expected: SERVED,
            ..Row::default()
        },
        Row {
            case: "a policy allowing only a WRITE action to every principal",
            name: "anon-policy-write",
            policy: Some(policy_document(&statement(
                "Allow",
                everyone,
                "s3:PutObject",
                &object_arn("anon-policy-write", "*"),
            ))),
            ..Row::default()
        },
        Row {
            case: "a policy allowing the read on ANOTHER key",
            name: "anon-policy-other",
            policy: Some(policy_document(&statement(
                "Allow",
                everyone,
                "s3:GetObject",
                &object_arn("anon-policy-other", "public/*"),
            ))),
            ..Row::default()
        },
        Row {
            case: "a public policy with RestrictPublicBuckets on",
            name: "anon-policy-restricted",
            block: Some([false, false, false, true]),
            policy: Some(policy_document(&statement(
                "Allow",
                everyone,
                "s3:GetObject",
                &object_arn("anon-policy-restricted", "*"),
            ))),
            ..Row::default()
        },
        Row {
            case: "a public policy beside an explicit Deny to every principal",
            name: "anon-policy-denied",
            policy: Some(policy_document(&format!(
                "{},{}",
                statement(
                    "Allow",
                    everyone,
                    "s3:GetObject",
                    &object_arn("anon-policy-denied", "*")
                ),
                statement(
                    "Deny",
                    everyone,
                    "s3:Get*",
                    &object_arn("anon-policy-denied", "*")
                ),
            ))),
            // The deny reaches EVERY principal, the bucket's owner included.
            signed_read_refused: true,
            ..Row::default()
        },
        Row {
            case: "a policy that grants the read to one account rather than to everyone",
            name: "anon-policy-account",
            policy: Some(policy_document(&statement(
                "Allow",
                "{\"AWS\":\"arn:aws:iam::123456789012:root\"}",
                "s3:GetObject",
                &object_arn("anon-policy-account", "*"),
            ))),
            ..Row::default()
        },
        Row {
            case: "a Requester Pays bucket holding a public-read object",
            name: "anon-requester-pays",
            ownership: Some("ObjectWriter"),
            object_acl: Some("public-read"),
            payer: Some("Requester"),
            expected: Expected {
                status: 403,
                code: ACCESS_DENIED,
                message: REQUESTER_PAYS_MESSAGE,
            },
            ..Row::default()
        },
    ]
}

#[tokio::test]
async fn an_unsigned_read_is_served_only_what_the_bucket_grants_to_everyone() {
    let router = router();

    for row in rows() {
        let bucket = configured_bucket(&router, &row).await;

        // Every endpoint host answers the same: where the request was addressed is not part
        // of what authorizes it.
        for host in ANONYMOUS_HOSTS {
            let read = send_anonymous(&router, "GET", host, &format!("/{bucket}/{KEY}")).await;
            assert_eq!(
                read.status, row.expected.status,
                "case {:?} at {host:?}: expected HTTP {}, got {} with body {:?}",
                row.case, row.expected.status, read.status, text(&read)
            );
            if row.expected.code.is_empty() {
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {:?} at {host:?}: a granted read must serve the stored bytes, got \
                     {:?}",
                    row.case,
                    text(&read)
                );
            } else {
                assert_eq!(
                    read.error_code(),
                    row.expected.code,
                    "case {:?} at {host:?}: expected the {:?} code, got {:?} with body {:?}",
                    row.case,
                    row.expected.code,
                    read.error_code(),
                    text(&read)
                );
                assert_eq!(
                    read.error_message(),
                    row.expected.message,
                    "case {:?} at {host:?}: expected the published message, got {:?}",
                    row.case,
                    read.error_message()
                );
            }

            // A HEAD of the same object is authorized by the same permission, and carries no
            // body to say so with — so the STATUS is the whole answer.
            let head = send_anonymous(&router, "HEAD", host, &format!("/{bucket}/{KEY}")).await;
            assert_eq!(
                head.status, row.expected.status,
                "case {:?} at {host:?}: an unsigned HEAD must answer the status its GET \
                 does, expected {}, got {}",
                row.case, row.expected.status, head.status
            );
        }

        // The bucket owner's own SIGNED read is unaffected by every row above except the one
        // whose policy DENIES every principal: the other refusals are about the caller
        // presenting no credential, not about the object being unreadable, while a
        // resource-based explicit deny reaches the owner too.
        let signed = get_object(&router, bucket, KEY, "", &[]).await;
        if row.signed_read_refused {
            assert_eq!(
                signed.status, 403,
                "case {:?}: a policy denying every principal must refuse the owner's signed \
                 read too, got {} with body {:?}",
                row.case,
                signed.status,
                text(&signed)
            );
            assert_eq!(
                signed.error_code(),
                ACCESS_DENIED,
                "case {:?}: expected the {ACCESS_DENIED:?} code, got {:?}",
                row.case,
                signed.error_code()
            );
            continue;
        }
        assert_eq!(
            signed.status, 200,
            "case {:?}: the owner's signed read must still be served, got {} with body {:?}",
            row.case,
            signed.status,
            text(&signed)
        );
        assert_eq!(
            signed.body.as_ref(),
            BODY,
            "case {:?}: the owner's signed read must serve the stored bytes, got {:?}",
            row.case,
            text(&signed)
        );
    }
}

#[tokio::test]
async fn an_unsigned_read_is_refused_before_it_says_whether_the_key_is_there() {
    let router = router();
    let bucket = "anon-absent-key";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let stored = put_object(&router, bucket, KEY, BODY, &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // A key the bucket holds and one it does not are answered IDENTICALLY to a caller
    // nothing grants the read to: the refusal is about the caller, so it reveals nothing
    // about which keys exist. The owner's signed read is what tells the two apart.
    for (case, key, signed_status) in [
        ("a key the bucket holds", KEY, 200),
        ("a key the bucket does not hold", "absent.csv", 404),
    ] {
        let read = send_anonymous(&router, "GET", "localhost", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            read.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?} with body {:?}",
            read.error_code(),
            text(&read)
        );
        let signed = get_object(&router, bucket, key, "", &[]).await;
        assert_eq!(
            signed.status, signed_status,
            "case {case:?}: the owner's signed read must answer HTTP {signed_status}, got {} \
             with body {:?}",
            signed.status,
            text(&signed)
        );
    }
}

/// RULE, at the other signature posture: with request authentication ON — as the emulator
/// runs it unless started otherwise — a request presenting no credential is refused by the
/// signature boundary before any handler sees it, and S3 words THAT refusal the same way:
/// the `AccessDenied` of its published error list, HTTP 403, with that list's own sentence.
///
/// What this pins is that an unsigned caller is answered in S3's own words rather than in
/// the framework's "sign the request" diagnostic, which no S3 client reads: a client
/// branching on `AccessDenied` and on the "Access Denied" message must take the same branch
/// whichever posture the emulator is run in. The rows differ along the method, the address
/// form (path-style and virtual-hosted-style) and whether the key exists.
#[tokio::test]
async fn an_unsigned_read_is_refused_in_s3_own_words_with_signatures_enforced() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut enforcing = MultiServiceRouter::new();
    // The fixture is written with signature enforcement off — standing up a bucket is not
    // what this test is about — and the request under test is dispatched with it on.
    enforcing.set_request_authentication(false);
    enforcing.register_service(EmulatedService::new(engine));
    let bucket = "anon-enforced";
    let created = create_bucket(&enforcing, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let stored = put_object(&enforcing, bucket, KEY, BODY, &[]).await;
    assert_eq!(
        stored.status, 200,
        "the object must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    enforcing.set_request_authentication(true);

    for (case, method, host, uri) in [
        (
            "a path-style read of a stored key",
            "GET",
            "localhost",
            format!("/{bucket}/{KEY}"),
        ),
        (
            "a path-style read of an absent key",
            "GET",
            "localhost",
            format!("/{bucket}/absent.csv"),
        ),
        (
            "a host-style read of a stored key",
            "GET",
            "anon-enforced.s3.us-east-1.amazonaws.com",
            format!("/{KEY}"),
        ),
        (
            "a path-style HEAD of a stored key",
            "HEAD",
            "localhost",
            format!("/{bucket}/{KEY}"),
        ),
    ] {
        let refused = send_anonymous(&enforcing, method, host, &uri).await;
        assert_eq!(
            refused.status, 403,
            "case {case:?}: expected HTTP 403, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        if method == "HEAD" {
            continue;
        }
        assert_eq!(
            refused.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?} with body {:?}",
            refused.error_code(),
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            ACCESS_DENIED_MESSAGE,
            "case {case:?}: expected the published message, got {:?}",
            refused.error_message()
        );
    }
}

#[tokio::test]
async fn an_unsigned_read_of_a_bucket_this_emulator_does_not_hold_is_not_served() {
    let router = router();

    // The boundary of the routing half: an unsigned request addressing a bucket nothing
    // here holds names no service in a credential and no bucket in the store, so S3 does
    // not claim it — and the one that DOES name a held bucket is claimed and answered by S3
    // itself, which is what the rows above pin.
    let unheld = send_anonymous(&router, "GET", "localhost", "/anon-no-such-bucket/key").await;
    assert_ne!(
        unheld.error_code(),
        ACCESS_DENIED,
        "an unsigned read of a bucket nothing holds cannot be answered by S3's \
         authorization, got {:?} with status {}",
        unheld.error_code(),
        unheld.status
    );

    let bucket = "anon-held-bucket";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let read = send_anonymous(&router, "GET", "localhost", &format!("/{bucket}/{KEY}")).await;
    assert_eq!(
        read.error_code(),
        ACCESS_DENIED,
        "an unsigned read of a bucket this emulator holds must be answered by S3's own \
         authorization, got {:?} with status {} and body {:?}",
        read.error_code(),
        read.status,
        text(&read)
    );
}

/// The acknowledgement header a client puts on a request it expects to be charged for, and
/// the value `RequestPayer` takes (`smithy_query(query="shape",shape="RequestPayer")`).
const REQUEST_PAYER_HEADER: (&str, &str) = ("x-amz-request-payer", "requester");

/// The Block Public Access configuration a bucket must be in for a PUBLIC bucket policy to
/// take effect while its ACL guardrails stay on: the policy switch off, the ACL ones on
/// ([`SETTINGS`] order).
const PUBLIC_POLICY_ALLOWED: Flags = [true, true, false, false];

/// One unsigned request against a bucket configured for a particular payer, and the sentence
/// its refusal must carry.
struct PayerRow {
    /// The case name every assertion of the row reports.
    case: &'static str,
    /// The bucket the request addresses.
    bucket: &'static str,
    /// The method it asks with.
    method: &'static str,
    /// The endpoint host it is addressed to.
    host: String,
    /// The request URI, in whichever address form `host` implies.
    uri: String,
    /// The headers it carries.
    headers: Vec<(&'static str, &'static str)>,
    /// The `<Message>` the refusal must carry.
    message: &'static str,
}

/// Stand up `name` holding one object every principal may read by policy, then put each
/// `payers` entry on it in turn — so a bucket whose payer was CHANGED is judged by the value
/// it carries now.
async fn public_bucket_paid_by(router: &MultiServiceRouter, name: &'static str, payers: &[&str]) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "bucket {name:?}: the bucket must be created first, got {} with body {:?}",
        created.status,
        text(&created)
    );
    let lifted = put_public_access_block(router, name, &members(PUBLIC_ACCESS_ALLOWED), &[]).await;
    assert_eq!(
        lifted.status, 200,
        "bucket {name:?}: Block Public Access must be liftable, got {} with body {:?}",
        lifted.status,
        text(&lifted)
    );
    let stored = put_object(router, name, KEY, BODY, &[]).await;
    assert_eq!(
        stored.status, 200,
        "bucket {name:?}: the object must be written first, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let policy = put_bucket_policy(
        router,
        name,
        &policy_document(&statement(
            "Allow",
            "\"*\"",
            "s3:GetObject",
            &object_arn(name, KEY),
        )),
    )
    .await;
    assert_eq!(
        policy.status, 204,
        "bucket {name:?}: the public-read policy must be stored, got {} with body {:?}",
        policy.status,
        text(&policy)
    );
    let blocked = put_public_access_block(router, name, &members(PUBLIC_POLICY_ALLOWED), &[]).await;
    assert_eq!(
        blocked.status, 200,
        "bucket {name:?}: the Block Public Access configuration must be stored, got {} with \
         body {:?}",
        blocked.status,
        text(&blocked)
    );
    for payer in payers {
        let put = put_request_payment(router, name, payer).await;
        assert_eq!(
            put.status, 200,
            "bucket {name:?}: the {payer:?} request payment configuration must be stored, got \
             {} with body {:?}",
            put.status,
            text(&put)
        );
    }
}

/// The bucket every row of [`payer_rows`] addresses: one that makes the REQUESTER pay, one
/// whose payer was set to `Requester` and then back to the owner, and one nobody has
/// configured a payer on at all.
const REQUESTER_PAYS_BUCKET: &str = "anon-payer-requester";
/// See [`REQUESTER_PAYS_BUCKET`].
const PAYER_RESET_BUCKET: &str = "anon-payer-reset";
/// See [`REQUESTER_PAYS_BUCKET`].
const PAYER_DEFAULT_BUCKET: &str = "anon-payer-default";

/// The table: one unsigned request per row, and the sentence its refusal must carry.
fn payer_rows() -> Vec<PayerRow> {
    let path_style = |bucket: &str| format!("/{bucket}/{KEY}");
    let mut rows = vec![
        PayerRow {
            case: "a Requester Pays bucket, read with no acknowledgement",
            bucket: REQUESTER_PAYS_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: path_style(REQUESTER_PAYS_BUCKET),
            headers: vec![],
            message: REQUESTER_PAYS_MESSAGE,
        },
        PayerRow {
            case: "a Requester Pays bucket, read WITH the acknowledgement header",
            bucket: REQUESTER_PAYS_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: path_style(REQUESTER_PAYS_BUCKET),
            headers: vec![REQUEST_PAYER_HEADER],
            message: REQUESTER_PAYS_MESSAGE,
        },
        PayerRow {
            case: "a Requester Pays bucket, addressed virtual-hosted-style",
            bucket: REQUESTER_PAYS_BUCKET,
            method: "GET",
            host: format!("{REQUESTER_PAYS_BUCKET}.s3.us-east-1.amazonaws.com"),
            uri: format!("/{KEY}"),
            headers: vec![],
            message: REQUESTER_PAYS_MESSAGE,
        },
        PayerRow {
            case: "a Requester Pays bucket, read at a key it does not hold",
            bucket: REQUESTER_PAYS_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: format!("/{REQUESTER_PAYS_BUCKET}/absent.csv"),
            headers: vec![],
            message: REQUESTER_PAYS_MESSAGE,
        },
        PayerRow {
            case: "a Requester Pays bucket, listed rather than read",
            bucket: REQUESTER_PAYS_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: format!("/{REQUESTER_PAYS_BUCKET}"),
            headers: vec![],
            message: REQUESTER_PAYS_MESSAGE,
        },
        PayerRow {
            case: "a bucket whose payer was reset to the bucket owner",
            bucket: PAYER_RESET_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: path_style(PAYER_RESET_BUCKET),
            headers: vec![],
            message: ACCESS_DENIED_MESSAGE,
        },
        PayerRow {
            case: "a bucket whose payer was reset, read WITH the acknowledgement header",
            bucket: PAYER_RESET_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: path_style(PAYER_RESET_BUCKET),
            headers: vec![REQUEST_PAYER_HEADER],
            message: ACCESS_DENIED_MESSAGE,
        },
        PayerRow {
            case: "a bucket nobody has configured a payer on",
            bucket: PAYER_DEFAULT_BUCKET,
            method: "GET",
            host: "localhost".to_string(),
            uri: path_style(PAYER_DEFAULT_BUCKET),
            headers: vec![],
            message: ACCESS_DENIED_MESSAGE,
        },
    ];
    // The same requests again as HEADs, which are authorized by the same permission and
    // carry no document to state the refusal in — so only their STATUS is read below.
    let heads: Vec<PayerRow> = rows
        .iter()
        .map(|row| PayerRow {
            case: row.case,
            bucket: row.bucket,
            method: "HEAD",
            host: row.host.clone(),
            uri: row.uri.clone(),
            headers: row.headers.clone(),
            message: row.message,
        })
        .collect();
    rows.extend(heads);
    rows
}

/// RULE: a request carrying NO credential names no account to charge, so a bucket that makes
/// the REQUESTER pay refuses it with its own fixed sentence — nothing in it taken from the
/// request or from state — instead of with the authorization refusal S3 answers a caller a
/// bucket grants nothing to. Both carry HTTP 403 and the `AccessDenied` code, which is why
/// the sentence is the whole difference between "fix the grant" and "authenticate and
/// acknowledge the charge"; and it holds wherever the unsigned caller is refused, so with
/// request authentication ON — as the emulator runs it unless started otherwise, which is
/// the posture this test drives — the signature boundary words it the same way an
/// operation's own authorization does.
///
/// The rows differ along who pays (`Requester`, a payer reset to the owner, a bucket with no
/// payer configured at all), whether the request acknowledges the charge, which method asks,
/// whether the address is path-style or virtual-hosted-style, whether the key is one the
/// bucket holds, and whether the request addresses an object or the bucket itself.
#[tokio::test]
async fn an_unsigned_request_against_a_requester_pays_bucket_is_refused_in_its_own_words() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut enforcing = MultiServiceRouter::new();
    // The fixture is written with signature enforcement off — standing up the buckets is not
    // what this test is about — and every request under test is dispatched with it on.
    enforcing.set_request_authentication(false);
    enforcing.register_service(EmulatedService::new(engine));
    public_bucket_paid_by(&enforcing, REQUESTER_PAYS_BUCKET, &["Requester"]).await;
    public_bucket_paid_by(&enforcing, PAYER_RESET_BUCKET, &["Requester", "BucketOwner"]).await;
    public_bucket_paid_by(&enforcing, PAYER_DEFAULT_BUCKET, &[]).await;
    enforcing.set_request_authentication(true);

    for row in payer_rows() {
        let refused =
            send_anonymous_with(&enforcing, row.method, &row.host, &row.uri, &row.headers).await;
        assert_eq!(
            refused.status, 403,
            "case {:?} ({} {}): expected HTTP 403, got {} with body {:?}",
            row.case,
            row.method,
            row.uri,
            refused.status,
            text(&refused)
        );
        if row.method == "HEAD" {
            continue;
        }
        assert_eq!(
            refused.error_code(),
            ACCESS_DENIED,
            "case {:?} ({} {}): expected the {ACCESS_DENIED:?} code, got {:?} with body {:?}",
            row.case,
            row.method,
            row.uri,
            refused.error_code(),
            text(&refused)
        );
        assert_eq!(
            refused.error_message(),
            row.message,
            "case {:?} ({} {}): expected {:?}, got {:?}",
            row.case,
            row.method,
            row.uri,
            row.message,
            refused.error_message()
        );
    }
}

/// The SAME rule at the other signature posture: with request authentication off the unsigned
/// request reaches the handler, whose own authorization refuses it — and a Requester Pays
/// bucket's refusal carries the identical sentence there, because what refuses the caller is
/// that it names no account to charge rather than where the refusal was decided.
///
/// The rows differ along the payer the bucket carries, whether the request acknowledges the
/// charge, and which method asks.
#[tokio::test]
async fn a_requester_pays_refusal_is_worded_the_same_wherever_it_is_decided() {
    let router = router();
    public_bucket_paid_by(&router, "anon-payer-handler", &["Requester"]).await;
    public_bucket_paid_by(&router, "anon-payer-handler-owner", &["BucketOwner"]).await;

    for (case, bucket, headers, expected) in [
        (
            "a Requester Pays bucket, read with no acknowledgement",
            "anon-payer-handler",
            &[][..],
            Some(REQUESTER_PAYS_MESSAGE),
        ),
        (
            "a Requester Pays bucket, read WITH the acknowledgement header",
            "anon-payer-handler",
            &[REQUEST_PAYER_HEADER][..],
            Some(REQUESTER_PAYS_MESSAGE),
        ),
        (
            "a bucket the OWNER pays for, whose policy grants the read to everyone",
            "anon-payer-handler-owner",
            &[][..],
            None,
        ),
        (
            "a bucket the OWNER pays for, read WITH the acknowledgement header",
            "anon-payer-handler-owner",
            &[REQUEST_PAYER_HEADER][..],
            None,
        ),
    ] {
        let read = send_anonymous_with(
            &router,
            "GET",
            "localhost",
            &format!("/{bucket}/{KEY}"),
            headers,
        )
        .await;
        // The HEAD of the same object, which the same permission authorizes — driven for a
        // row that does NOT carry the acknowledgement header, because `HeadObject` refuses
        // that member with its own "not served here" refusal before any authorization
        // question, which is its own business rather than this rule's.
        let head = headers.is_empty().then_some(
            send_anonymous_with(&router, "HEAD", "localhost", &format!("/{bucket}/{KEY}"), &[])
                .await,
        );
        match expected {
            Some(message) => {
                assert_eq!(
                    read.status, 403,
                    "case {case:?}: expected HTTP 403, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.error_code(),
                    ACCESS_DENIED,
                    "case {case:?}: expected the {ACCESS_DENIED:?} code, got {:?} with body \
                     {:?}",
                    read.error_code(),
                    text(&read)
                );
                assert_eq!(
                    read.error_message(),
                    message,
                    "case {case:?}: expected {message:?}, got {:?}",
                    read.error_message()
                );
            }
            // The control: the byte-identical request against a bucket whose OWNER pays is
            // SERVED, which is what isolates Requester Pays as the cause of the refusals
            // above rather than the grant.
            None => {
                assert_eq!(
                    read.status, 200,
                    "case {case:?}: expected the read to be served, got {} with body {:?}",
                    read.status,
                    text(&read)
                );
                assert_eq!(
                    read.body.as_ref(),
                    BODY,
                    "case {case:?}: a granted read must serve the stored bytes, got {:?}",
                    text(&read)
                );
            }
        }
        if let Some(head) = head {
            assert_eq!(
                head.status,
                read.status,
                "case {case:?}: an unsigned HEAD must answer the status its GET does, expected \
                 {}, got {}",
                read.status,
                head.status
            );
        }
    }
}
