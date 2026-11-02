// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two things a client stages on one bucket TOGETHER: an object
//! and the bucket policy that grants access to it.
//!
//! The object suites in this crate own `PutObject`/`GetObject`/`HeadObject` and the policy
//! suite owns `PutBucketPolicy`/`GetBucketPolicy`; none of them drives the two against the
//! same bucket, which is exactly what a data-plane client does before it reads anything
//! (create the bucket, attach a policy, upload the document). That pairing is what this
//! suite adds, and the rule it pins is their INDEPENDENCE: a bucket's policy and its
//! objects are separate subresources, so attaching or replacing one leaves the other byte
//! for byte — the policy document is not re-serialized by an object write, and an object's
//! bytes and entity tag are not touched by a policy write.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! the bucket-lifecycle suite in this crate already owns), so what is pinned is the wire
//! behaviour rather than any handler's internals.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{
    create_bucket, get_bucket_policy, put_bucket_policy, router, send_bytes, Answer,
};

/// `PUT /<bucket>/<key>` — `PutObject`, with the object's bytes as the body.
async fn put_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "PUT", &format!("/{bucket}/{key}"), headers, body).await
}

/// `GET /<bucket>/<key>` — `GetObject`.
async fn get_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send_bytes(router, "GET", &format!("/{bucket}/{key}"), &[], b"").await
}

#[tokio::test]
async fn a_bucket_policy_and_objects_coexist_on_one_bucket_without_touching_each_other() {
    let router = router();
    let bucket = "staging-bucket";
    let created = create_bucket(&router, bucket, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );

    // The staging order a data-plane client uses: attach the policy that grants a service
    // principal access, then upload the document it will read. The document is indented
    // JSON with newlines and tabs, which is what makes "verbatim" testable here — a
    // policy re-serialized on the way in or out would come back without this whitespace.
    let policy = "{\n\t\"Version\": \"2012-10-17\",\n\t\"Statement\": [\n\t\t{\n\t\t\t\"Sid\": \
                  \"AllowServiceAccess\",\n\t\t\t\"Effect\": \"Allow\",\n\t\t\t\"Principal\": \
                  {\n\t\t\t\t\"Service\": \"example.amazonaws.com\"\n\t\t\t},\n\t\t\t\"Action\": \
                  [\n\t\t\t\t\"s3:GetObject\",\n\t\t\t\t\"s3:PutObject\"\n\t\t\t],\n\t\t\t\
                  \"Resource\": \"arn:aws:s3:::staging-bucket/*\"\n\t\t}\n\t]\n}";
    let put_policy = put_bucket_policy(&router, bucket, policy).await;
    assert_eq!(
        put_policy.status, 204,
        "the policy must be attached with 204 No Content, got {} with body {:?}",
        put_policy.status,
        String::from_utf8_lossy(&put_policy.body)
    );

    let key = "in/sample.edi";
    let document: &[u8] = b"ISA*00*          *00*          *ZZ*SENDERID       *ZZ*RECEIVERID    *\
        240101*1200*U*00401*000000001*0*P*>~GS*PO*SENDER*RECEIVER*20240101*1200*1*X*004010~\
        ST*850*0001~SE*2*0001~GE*1*1~IEA*1*000000001~";
    let put_document = put_object(
        &router,
        bucket,
        key,
        document,
        &[("content-type", "application/EDI-X12")],
    )
    .await;
    assert_eq!(
        put_document.status, 200,
        "the object must be stored in the bucket that carries the policy, got {} with \
         body {:?}",
        put_document.status,
        String::from_utf8_lossy(&put_document.body)
    );

    // Both are readable, and each reports what was written.
    let read_object = get_object(&router, bucket, key).await;
    assert_eq!(
        read_object.body.as_ref(),
        document,
        "the object must read back byte for byte, got {} bytes",
        read_object.body.len()
    );
    assert_eq!(
        read_object.header("content-type").as_deref(),
        Some("application/EDI-X12"),
        "the object must keep the Content-Type the write set, got {:?}",
        read_object.header("content-type")
    );
    let read_policy = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        read_policy.status, 200,
        "the policy must read back, got {} with body {:?}",
        read_policy.status,
        String::from_utf8_lossy(&read_policy.body)
    );
    assert_eq!(
        String::from_utf8_lossy(&read_policy.body),
        policy,
        "the policy must read back verbatim, got {:?}",
        String::from_utf8_lossy(&read_policy.body)
    );

    // Replacing the policy leaves the object alone. The replacement is another `Allow`,
    // deliberately not a `Deny`: a bucket policy is in force on every caller, so an explicit
    // deny would refuse the reads below — which is authorization, not the independence of the
    // two subresources this suite is about.
    let replacement =
        r#"{"Version":"2012-10-17","Statement":[{"Sid":"AllowDelivery","Effect":"Allow","Principal":{"Service":"logs.amazonaws.com"},"Action":"s3:PutObject","Resource":"arn:aws:s3:::staging-bucket/*"}]}"#;
    let replaced = put_bucket_policy(&router, bucket, replacement).await;
    assert_eq!(
        replaced.status, 204,
        "the replacing policy must be accepted with 204 No Content, got {} with body {:?}",
        replaced.status,
        String::from_utf8_lossy(&replaced.body)
    );
    let reread_object = get_object(&router, bucket, key).await;
    assert_eq!(
        reread_object.body.as_ref(),
        document,
        "replacing the policy must not touch the stored object, got {} bytes",
        reread_object.body.len()
    );
    assert_eq!(
        reread_object.header("etag"),
        read_object.header("etag"),
        "replacing the policy must not change the object's ETag, got {:?}",
        reread_object.header("etag")
    );

    // ... and writing another object leaves the policy alone.
    let second = put_object(&router, bucket, "in/second.edi", b"ISA*00*~IEA*1*2~", &[]).await;
    assert_eq!(
        second.status, 200,
        "the second object must be stored, got {} with body {:?}",
        second.status,
        String::from_utf8_lossy(&second.body)
    );
    let reread_policy = get_bucket_policy(&router, bucket).await;
    assert_eq!(
        String::from_utf8_lossy(&reread_policy.body),
        replacement,
        "writing an object must not touch the bucket's policy, got {:?}",
        String::from_utf8_lossy(&reread_policy.body)
    );
    // And the first object is still there beside the second: an object write replaces
    // only its own key.
    let first_again = get_object(&router, bucket, key).await;
    assert_eq!(
        first_again.body.as_ref(),
        document,
        "writing a second object must leave the first one as it was, got {} bytes",
        first_again.body.len()
    );
}
