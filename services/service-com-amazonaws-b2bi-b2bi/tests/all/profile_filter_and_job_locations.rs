// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two references a b2bi request carries WITHOUT them being the
//! resource it addresses: the `profileId` a `ListPartnerships` page is narrowed to, and
//! the Amazon S3 buckets a transformer job reads from and writes to. Both are driven over
//! the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`) through the same router the
//! emulator boots, with the emulator's own S3 service mounted beside b2bi.
//!
//! The rules under test:
//!
//! * a listing that narrows to a `profileId` RESOLVES it before it enumerates anything: a
//!   `profileId` this account's store holds no profile for is the modelled
//!   `ResourceNotFoundException`, never a successful empty page — which would tell the
//!   caller its stale id names a profile that merely has no partnerships;
//! * a `StartTransformerJob` location has to name a bucket that is in the Region the call
//!   is signed for — the Region of the S3 endpoint the job's reads and writes go to — and
//!   a bucket that is not is the modelled `ValidationException`, raised before anything is
//!   read or written.
//!
//! The rows span what each rule generalises over: for the filter, how the id fails to
//! resolve (never minted, deleted, holding an id of another resource kind) and which
//! paging members travel with it; for the locations, which member carries the bucket
//! (`inputFile` or `outputLocation`), which Region the bucket is in, whether it exists at
//! all, and which Region the CALL is signed for — the same request is served in one Region
//! and refused in another.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::scaffolded::NOT_FOUND_STATUS;
use service_com_amazonaws_b2bi_b2bi::state::UNCONSTRAINED_BUCKET_REGION;
use smithy_server_runtime::MultiServiceRouter;

use crate::edi_parsing::{given_object, BUCKET, X12_850};
use crate::onboarding_loop::{
    b2bi_router, call, call_signed_for, error_code, given_capability, given_partnership,
    given_profile, given_transformer, s3, s3_signed_for, text, SIGNING_REGION,
};
use crate::transformer_jobs::{await_job, set_status};

/// A Region that is neither the one these calls are signed for nor the one an
/// unconstrained bucket lives in, so a bucket created in it is "somewhere else" from both.
const FAR_REGION: &str = "eu-west-1";

/// The prefix this suite's jobs are asked to write their output file under.
const OUTPUT_PREFIX: &str = "out/locations/";

/// The partnership ids a listing reports, in the order it reports them.
fn listed_partnerships(page: &Value) -> Vec<String> {
    page["partnerships"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| {
                    summary["partnershipId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a `ListPartnerships` call that names a `profileId` resolves that profile before
/// it builds a page. An id no profile holds — however it fails to resolve, and whatever
/// paging members travel with it — is the modelled `ResourceNotFoundException` naming the
/// id from the request; an id that does resolve pages exactly that profile's partnerships.
#[tokio::test]
async fn list_partnerships_resolves_its_profile_filter_before_it_builds_a_page() {
    let router = b2bi_router();
    let capability_id = given_capability(&router, "filter-capability").await;
    let busy_profile = given_profile(&router, "filter-busy").await;
    let quiet_profile = given_profile(&router, "filter-quiet").await;
    let doomed_profile = given_profile(&router, "filter-doomed").await;
    let first = given_partnership(&router, &busy_profile, &capability_id, "filter-one").await;
    let second = given_partnership(&router, &busy_profile, &capability_id, "filter-two").await;

    let (status, body) = call(
        &router,
        "DeleteProfile",
        json!({ "profileId": doomed_profile.clone() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the profile under test must be deletable, got {status} {body}"
    );

    // A live page token, so the row that sends one is refused by the FILTER rather than by
    // the token check.
    let (status, page) = call(&router, "ListPartnerships", json!({ "maxResults": 1 })).await;
    assert_eq!(
        status, 200,
        "an unfiltered first page must succeed, got {status} {page}"
    );
    let live_token = page["nextToken"]
        .as_str()
        .expect("a second page of two partnerships")
        .to_string();

    // Well-formed per the modelled `len 1..64` / `^[a-zA-Z0-9_-]+$` on `ProfileId`, so
    // what refuses it is the lookup rather than the boundary validator.
    let never_minted = "p-0123456789abcdef0";

    // Each row: the case, the `profileId` that resolves to nothing, and the other members
    // the request carries.
    let cases: Vec<(&str, String, Value)> = vec![
        (
            "a never-minted profileId",
            never_minted.to_string(),
            json!({}),
        ),
        (
            "a profileId whose profile was deleted",
            doomed_profile.clone(),
            json!({}),
        ),
        (
            "an id a capability really holds",
            capability_id.clone(),
            json!({}),
        ),
        (
            "an id a partnership really holds",
            first.clone(),
            json!({}),
        ),
        (
            "a never-minted profileId carrying a page size",
            never_minted.to_string(),
            json!({ "maxResults": 1 }),
        ),
        (
            "a never-minted profileId carrying a live page token",
            never_minted.to_string(),
            json!({ "nextToken": live_token }),
        ),
    ];

    for (case, profile_id, extra) in cases {
        let mut request = json!({ "profileId": profile_id.clone() });
        for (member, value) in extra.as_object().expect("an object of members") {
            request[member] = value.clone();
        }
        let (status, body) = call(&router, "ListPartnerships", request).await;
        assert_eq!(
            status, NOT_FOUND_STATUS,
            "case {case:?}: a filter naming no profile is refused with \
             {NOT_FOUND_STATUS}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
        // `ResourceNotFoundException`'s message member is `message`
        // (`smithy_query(query="shape",shape="ResourceNotFoundException")`).
        assert_eq!(
            text(&body, "message"),
            format!("Profile with profileId {profile_id} not found."),
            "case {case:?}: expected the refusal to name the profileId from the request, \
             got {:?}",
            text(&body, "message")
        );
        assert!(
            body["partnerships"].is_null(),
            "case {case:?}: a refused listing carries no page, got {body}"
        );
    }

    // And the filter still selects: each row is the case, the request, and the
    // partnerships the page must hold, in creation order.
    let resolving: Vec<(&str, Value, Vec<String>)> = vec![
        (
            "the profile that holds both partnerships",
            json!({ "profileId": busy_profile.clone() }),
            vec![first.clone(), second.clone()],
        ),
        (
            "a live profile that holds none",
            json!({ "profileId": quiet_profile.clone() }),
            Vec::new(),
        ),
        (
            "the profile that holds both, one page at a time",
            json!({ "profileId": busy_profile.clone(), "maxResults": 1 }),
            vec![first.clone()],
        ),
        (
            "no profileId at all",
            json!({}),
            vec![first.clone(), second.clone()],
        ),
    ];

    for (case, request, expected) in resolving {
        let (status, page) = call(&router, "ListPartnerships", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a listing that resolves must succeed, got {status} {page}"
        );
        assert_eq!(
            listed_partnerships(&page),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            listed_partnerships(&page)
        );
    }
}

/// Create `bucket` in `region` through the emulator's own Amazon S3 service.
///
/// S3 serves a create that names no `LocationConstraint` only at the endpoint the
/// unspecified constraint stands for, so a bucket anywhere else names its own Region and
/// the create is signed for it.
async fn given_bucket(router: &MultiServiceRouter, region: &str, bucket: &str) {
    let configuration = if region == UNCONSTRAINED_BUCKET_REGION {
        Vec::new()
    } else {
        format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{region}</LocationConstraint>\
             </CreateBucketConfiguration>"
        )
        .into_bytes()
    };
    let (status, response) =
        s3_signed_for(router, region, "PUT", &format!("/{bucket}"), &configuration).await;
    assert!(
        status == 200 || status == 409,
        "creating s3://{bucket} in {region} must succeed or already exist, got {status} {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// Store `body` at `bucket`/`key` in the bucket's own Region.
async fn given_object_in(
    router: &MultiServiceRouter,
    region: &str,
    bucket: &str,
    key: &str,
    body: &str,
) {
    let (status, response) = s3_signed_for(
        router,
        region,
        "PUT",
        &format!("/{bucket}/{key}"),
        body.as_bytes(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "storing s3://{bucket}/{key} must succeed, got {status} {:?}",
        String::from_utf8_lossy(&response)
    );
}

/// The `StartTransformerJob` request for `transformer_id` over `input_bucket`/`key`,
/// writing under [`OUTPUT_PREFIX`] of `output_bucket`.
fn start_request(
    transformer_id: &str,
    input_bucket: &str,
    key: &str,
    output_bucket: &str,
) -> Value {
    json!({
        "transformerId": transformer_id,
        "inputFile": { "bucketName": input_bucket, "key": key },
        "outputLocation": { "bucketName": output_bucket, "key": OUTPUT_PREFIX }
    })
}

/// RULE: every Amazon S3 location a transformer job names has to be in the Region the call
/// is signed for. A bucket that is not — held in another Region, or holding nothing
/// anywhere — is refused with the modelled `ValidationException` naming the location, the
/// bucket and that Region, before anything is read or written; a request whose locations
/// are both in the call's own Region runs and writes its output file there.
#[tokio::test]
async fn a_transformer_job_reads_and_writes_only_in_the_region_the_call_is_signed_for() {
    let router = b2bi_router();

    // The home bucket lives in the Region these calls are signed for by default.
    let home_key = "inbound/locations-home.edi";
    given_object(&router, home_key, X12_850).await;

    // Two buckets elsewhere, one of them holding a perfectly readable input file: what
    // refuses a job over it is the Region, not a missing object.
    let abroad = "b2bi-job-bucket-abroad";
    let abroad_key = "inbound/locations-abroad.edi";
    given_bucket(&router, UNCONSTRAINED_BUCKET_REGION, abroad).await;
    given_object_in(
        &router,
        UNCONSTRAINED_BUCKET_REGION,
        abroad,
        abroad_key,
        X12_850,
    )
    .await;
    let far = "b2bi-job-bucket-far";
    given_bucket(&router, FAR_REGION, far).await;

    // And a well-formed bucket name (the modelled `len 3..63`) created in no Region at all.
    let nowhere = "b2bi-job-bucket-nowhere";

    let transformer_id = given_transformer(&router, "locations").await;
    set_status(&router, &transformer_id, "active").await;

    // Each row: the case, the Region the call is signed for, the input bucket and key, the
    // output bucket, and the location/bucket pair the refusal must name.
    let refused: Vec<(&str, &str, &str, &str, &str, &str, &str)> = vec![
        (
            "an input bucket in another Region",
            SIGNING_REGION,
            abroad,
            abroad_key,
            BUCKET,
            "Input",
            abroad,
        ),
        (
            "an input bucket in a third Region",
            SIGNING_REGION,
            far,
            abroad_key,
            BUCKET,
            "Input",
            far,
        ),
        (
            "an input bucket that is in no Region at all",
            SIGNING_REGION,
            nowhere,
            home_key,
            BUCKET,
            "Input",
            nowhere,
        ),
        (
            "an output bucket in another Region",
            SIGNING_REGION,
            BUCKET,
            home_key,
            abroad,
            "Output",
            abroad,
        ),
        (
            "an output bucket that is in no Region at all",
            SIGNING_REGION,
            BUCKET,
            home_key,
            nowhere,
            "Output",
            nowhere,
        ),
        (
            // The SAME locations the suite's served rows use, signed for another Region:
            // the verdict follows the call's Region, not the bucket's name.
            "the home bucket, from a call signed for another Region",
            UNCONSTRAINED_BUCKET_REGION,
            BUCKET,
            home_key,
            BUCKET,
            "Input",
            BUCKET,
        ),
    ];

    for (case, region, input_bucket, key, output_bucket, role, named) in refused {
        let (status, body) = call_signed_for(
            &router,
            region,
            "StartTransformerJob",
            start_request(&transformer_id, input_bucket, key, output_bucket),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a location outside the call's Region is refused with 400, got \
             {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {body}"
        );
        // The modelled `ValidationException`'s message member is `Message`
        // (`smithy_query(query="shape",shape="ValidationException")`).
        assert_eq!(
            text(&body, "Message"),
            format!("{role} Bucket {named} is not in current region {region}"),
            "case {case:?}: expected the refusal to name the location, the bucket and the \
             call's Region, got {:?}",
            text(&body, "Message")
        );
        assert!(
            body["transformerJobId"].is_null(),
            "case {case:?}: a refused request starts no job, got {body}"
        );
    }

    // Nothing was read or written for any of those rows: the output prefix is empty.
    let (status, listed) = s3(
        &router,
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={OUTPUT_PREFIX}"),
        b"",
    )
    .await;
    let listing = String::from_utf8_lossy(&listed);
    assert_eq!(status, 200, "the listing must succeed, got {status}");
    assert!(
        !listing.contains("<Key>"),
        "a refused job must write nothing under {OUTPUT_PREFIX}, got {listing}"
    );

    // And a request whose locations ARE in the Region it is signed for runs: each row is
    // the case, that Region, the input bucket and key, and the output bucket.
    let served: Vec<(&str, &str, &str, &str, &str)> = vec![
        (
            "both locations in the Region the call is signed for",
            SIGNING_REGION,
            BUCKET,
            home_key,
            BUCKET,
        ),
        (
            "both locations in another Region, from a call signed for it",
            UNCONSTRAINED_BUCKET_REGION,
            abroad,
            abroad_key,
            abroad,
        ),
    ];

    for (case, region, input_bucket, key, output_bucket) in served {
        let (status, started) = call_signed_for(
            &router,
            region,
            "StartTransformerJob",
            start_request(&transformer_id, input_bucket, key, output_bucket),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the job must start, got {status} {started}"
        );
        let job_id = text(&started, "transformerJobId");
        assert_eq!(
            job_id.len(),
            25,
            "case {case:?}: the modelled TransformerJobId is 25 characters, got {job_id:?}"
        );
        let report = await_job(&router, &transformer_id, &job_id).await;
        assert_eq!(
            report["status"],
            json!("succeeded"),
            "case {case:?}: the job must succeed, got {report}"
        );
        assert_eq!(
            report["outputFiles"][0]["bucketName"],
            json!(output_bucket),
            "case {case:?}: the output file must be reported in the requested bucket, got \
             {report}"
        );
    }
}
