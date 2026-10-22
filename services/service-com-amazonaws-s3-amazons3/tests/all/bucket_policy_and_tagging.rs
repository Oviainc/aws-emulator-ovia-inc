// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for S3's bucket POLICY and TAGGING subresources: put one, read it
//! back, and get the right refusal when the bucket or the subresource is not there.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is
//! the wire behaviour (status, error code, payload) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. `PutBucketPolicy` stores the policy document verbatim and answers `204 No Content`
//!    with an empty body, so `GetBucketPolicy` returns the same bytes;
//!    `PutBucketTagging` REPLACES the tag set, so
//!    `GetBucketTagging` reports exactly the last submitted one and nothing of the one
//!    before it.
//! 2. A `Policy` that is not a policy document is `MalformedPolicy` (400) — judged
//!    before the bucket is resolved, so the verdict does not depend on the bucket.
//! 3. A policy read or write against a bucket this emulator does not hold is
//!    `NoSuchBucket` (404); a read of a policy a bucket does not carry is its own code
//!    (`NoSuchBucketPolicy`), which is a different state.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The wire error code: the `<Code>` element of S3's restXml error document (the
    /// model sets `noErrorWrapping`, so it is the document's own child).
    fn error_code(&self) -> String {
        self.body
            .split_once("<Code>")
            .and_then(|(_, rest)| rest.split_once("</Code>"))
            .map(|(code, _)| code.to_string())
            .unwrap_or_else(|| self.body.clone())
    }
}

/// Dispatch one request.
async fn send(router: &MultiServiceRouter, method: &str, uri: &str, body: &str) -> Answer {
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/xml")
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        body: String::from_utf8_lossy(&body).to_string(),
    }
}

/// A `Tagging` payload carrying `tags` in order.
fn tagging_payload(tags: &[(&str, &str)]) -> String {
    let entries: String = tags
        .iter()
        .map(|(key, value)| format!("<Tag><Key>{key}</Key><Value>{value}</Value></Tag>"))
        .collect();
    format!(
        "<Tagging xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\"><TagSet>{entries}</TagSet>\
         </Tagging>"
    )
}

/// A policy document granting `action` on `resource` to everyone.
fn policy(action: &str, resource: &str) -> String {
    serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": "*",
            "Action": action,
            "Resource": resource,
        }],
    })
    .to_string()
}

#[tokio::test]
async fn a_bucket_policy_round_trips_and_a_malformed_one_is_refused() {
    let router = router();
    let bucket = "harvest-destination";
    let created = send(&router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created, got {} with body {}",
        created.status, created.body
    );

    // Before any put, the bucket exists but carries no policy — a different state from
    // "no such bucket", and its own code.
    let answer = send(&router, "GET", &format!("/{bucket}?policy"), "").await;
    assert_eq!(
        answer.status, 404,
        "case \"a bucket with no policy\": expected HTTP 404, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchBucketPolicy",
        "case \"a bucket with no policy\": expected NoSuchBucketPolicy, got {:?}",
        answer.error_code()
    );

    // Accepted documents, differing in statement count, action form and principal form.
    // The last one put is what the read reports, byte for byte.
    let accepted: &[(&str, String)] = &[
        (
            "one statement, a single action",
            policy("s3:PutObject", &format!("arn:aws:s3:::{bucket}/*")),
        ),
        (
            "one statement, a wildcard action",
            policy("s3:*", &format!("arn:aws:s3:::{bucket}/*")),
        ),
        (
            "two statements, one of them a Deny",
            serde_json::json!({
                "Statement": [
                    {"Effect": "Allow", "Principal": "*", "Action": "s3:PutObject"},
                    {"Effect": "Deny", "Principal": "*", "Action": "s3:DeleteObject"},
                ],
            })
            .to_string(),
        ),
        (
            "a single statement object rather than a list",
            serde_json::json!({
                "Statement": {"Effect": "Allow", "Principal": "*", "Action": "s3:GetObject"},
            })
            .to_string(),
        ),
    ];
    for (label, document) in accepted {
        let answer = send(&router, "PUT", &format!("/{bucket}?policy"), document).await;
        assert_eq!(
            answer.status, 204,
            "case {label:?}: PutBucketPolicy must accept it with 204, got {} with body {}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_empty(),
            "case {label:?}: a 204 carries no body, got {:?}",
            answer.body
        );
        let read = send(&router, "GET", &format!("/{bucket}?policy"), "").await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetBucketPolicy must find the policy, got {} with body {}",
            read.status, read.body
        );
        assert_eq!(
            read.body,
            document.as_str(),
            "case {label:?}: the policy must come back byte for byte"
        );
    }

    // Documents that are not policy documents, differing in HOW they fail. Judged for
    // BOTH an existing bucket and an absent one: the verdict is the same, which is what
    // proves the validation precedes the lookup.
    let malformed: &[(&str, &str)] = &[
        ("not JSON at all", "{not json"),
        ("a JSON array", "[]"),
        ("a JSON number", "7"),
        ("no Statement member", "{\"Version\":\"2012-10-17\"}"),
        ("an empty Statement list", "{\"Statement\":[]}"),
        (
            "an Effect outside Allow and Deny",
            "{\"Statement\":[{\"Effect\":\"Maybe\",\"Action\":\"s3:GetObject\"}]}",
        ),
        (
            "a statement with no Action",
            "{\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":\"*\"}]}",
        ),
    ];
    for (label, document) in malformed {
        for target in [bucket, "no-such-bucket-at-all"] {
            let answer = send(&router, "PUT", &format!("/{target}?policy"), document).await;
            assert_eq!(
                answer.status, 400,
                "case {label:?} on {target:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "MalformedPolicy",
                "case {label:?} on {target:?}: expected MalformedPolicy, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the policy the last accepted put stored is still there: a refused put wrote
    // nothing.
    let read = send(&router, "GET", &format!("/{bucket}?policy"), "").await;
    assert_eq!(
        read.body,
        accepted.last().expect("a last accepted document").1,
        "a refused put must leave the stored policy alone"
    );
}

#[tokio::test]
async fn a_bucket_policy_past_the_published_size_limit_is_refused() {
    let router = router();
    let bucket = "harvest-destination";
    assert_eq!(
        send(&router, "PUT", &format!("/{bucket}"), "").await.status,
        200,
        "the bucket must be created"
    );

    // One row at the published ceiling and one past it. The ceiling is
    // `state::MAX_BUCKET_POLICY_BYTES` (20 KB), S3's published bucket-policy limit; the
    // document is padded in a `Sid` so it stays a valid policy document at both sizes.
    // The builder is the shared one, so this suite and the policy-status suite cannot
    // drift on what "exactly at the ceiling" means.
    let document = super::bucket_lifecycle::policy_of_size;

    let at_limit = document(20 * 1024);
    assert_eq!(
        at_limit.len(),
        20 * 1024,
        "the at-limit document must be exactly at the published ceiling"
    );
    let answer = send(&router, "PUT", &format!("/{bucket}?policy"), &at_limit).await;
    assert_eq!(
        answer.status, 204,
        "case \"a policy exactly at the 20 KB limit\": expected HTTP 204, got {} with body {}",
        answer.status, answer.body
    );

    for (label, size) in [
        ("one byte past the limit", 20 * 1024 + 1),
        ("ten times the limit", 200 * 1024),
    ] {
        let oversized = document(size);
        let answer = send(&router, "PUT", &format!("/{bucket}?policy"), &oversized).await;
        assert_eq!(
            answer.status, 400,
            "case {label:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "MalformedPolicy",
            "case {label:?}: expected MalformedPolicy, got {:?}",
            answer.error_code()
        );
    }

    // A refused put wrote nothing: the at-limit document is still the stored one.
    let read = send(&router, "GET", &format!("/{bucket}?policy"), "").await;
    assert_eq!(
        read.body, at_limit,
        "a refused put must leave the stored policy alone"
    );
}

#[tokio::test]
async fn a_bucket_tag_set_round_trips_and_a_put_replaces_it() {
    let router = router();
    let bucket = "harvest-destination";
    assert_eq!(
        send(&router, "PUT", &format!("/{bucket}"), "").await.status,
        200,
        "the bucket must be created"
    );

    // Before any put, the bucket exists but carries no tag set.
    let answer = send(&router, "GET", &format!("/{bucket}?tagging"), "").await;
    assert_eq!(
        answer.status, 404,
        "case \"a bucket with no tag set\": expected HTTP 404, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "NoSuchTagSet",
        "case \"a bucket with no tag set\": expected NoSuchTagSet, got {:?}",
        answer.error_code()
    );

    // Tag sets differing in size and in key shape. Each put REPLACES the previous one,
    // which is why the read after each is asserted against that row alone. An EMPTY
    // `TagSet` leaves the bucket with no tags associated — "it will overwrite any current
    // tags the bucket already has" (`PutBucketTagging`) — which `GetBucketTagging`
    // reports as `NoSuchTagSet`, so that row expects the refusal rather than a 200.
    let tag_sets: &[(&str, &[(&str, &str)])] = &[
        ("one tag", &[("team", "video")]),
        (
            "three tags",
            &[("team", "video"), ("env", "test"), ("cost-centre", "42")],
        ),
        ("an empty tag set", &[]),
        ("one tag again, after the empty one", &[("only", "one")]),
    ];
    for (label, tags) in tag_sets {
        let answer = send(
            &router,
            "PUT",
            &format!("/{bucket}?tagging"),
            &tagging_payload(tags),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {label:?}: PutBucketTagging must accept it, got {} with body {}",
            answer.status, answer.body
        );

        let read = send(&router, "GET", &format!("/{bucket}?tagging"), "").await;
        if tags.is_empty() {
            assert_eq!(
                read.error_code(),
                "NoSuchTagSet",
                "case {label:?}: a bucket left with no tags has no tag set, got {} with \
                 body {}",
                read.status, read.body
            );
            continue;
        }
        assert_eq!(
            read.status, 200,
            "case {label:?}: GetBucketTagging must report the tag set, got {} with body {}",
            read.status, read.body
        );
        for (key, value) in *tags {
            assert!(
                read.body.contains(&format!("<Key>{key}</Key>"))
                    && read.body.contains(&format!("<Value>{value}</Value>")),
                "case {label:?}: the tag set must report {key}={value}, got {}",
                read.body
            );
        }
        // And nothing the PREVIOUS put carried survives the replacement.
        for (_, previous) in tag_sets.iter() {
            for (key, _) in *previous {
                if tags.iter().any(|(kept, _)| kept == key) {
                    continue;
                }
                assert!(
                    !read.body.contains(&format!("<Key>{key}</Key>")),
                    "case {label:?}: a put replaces the tag set, so {key} must be gone, got {}",
                    read.body
                );
            }
        }
    }
}

#[tokio::test]
async fn every_bucket_subresource_refuses_an_absent_bucket() {
    let router = router();
    let absent = "no-such-bucket-at-all";
    let valid = policy("s3:GetObject", "*");

    let cases: &[(&str, &str, &str, &str)] = &[
        ("PutBucketPolicy", "PUT", "?policy", valid.as_str()),
        ("GetBucketPolicy", "GET", "?policy", ""),
    ];
    for (operation, method, query, body) in cases {
        let answer = send(&router, method, &format!("/{absent}{query}"), body).await;
        assert_eq!(
            answer.status, 404,
            "case {operation:?}: expected HTTP 404, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "NoSuchBucket",
            "case {operation:?}: expected NoSuchBucket, got {:?}",
            answer.error_code()
        );
    }
}
