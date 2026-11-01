// SPDX-License-Identifier: CC0-1.0

//! `ListS3Buckets` over the S3 buckets the account holds.
//!
//! The rule under test, from the operation's own documentation ("The list of S3 buckets
//! in your account", `S3BucketInfo$Name` "The name of the S3 bucket",
//! `S3BucketInfo$CreationDate` "The creation date of the S3 bucket"): the answer names
//! EVERY bucket Amazon S3 holds FOR THE ACCOUNT and nothing else, each with its name and
//! the instant S3 recorded it, in bucket-name order — whatever the bucket's name, Region
//! or creation order, and however many there are. A bucket another emulated service
//! provisioned as the object store of one of its own resources is not a bucket of the
//! account, so it is not named. `NextToken` is "Reserved for future use" on both the
//! request and the response, so it selects nothing and is never reported.
//!
//! Every bucket of the account here is created over S3's REAL wire (`PUT /<bucket>` —
//! `CreateBucket`) against the same shared engine both services are mounted on, exactly
//! as a client creates one. The one exception is the case about a bucket another emulated
//! service provisioned as the object store of its own resource: no S3 API creates such a
//! bucket, so that case reaches the S3 store's `provision_bucket` seam — the one the
//! owning service's control plane calls — directly. That is also what the operation being
//! a REPORTING one means here: a second `ListS3Buckets` with no intervening S3 call
//! answers the identical document.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The endpoint host an AmplifyBackend request addresses.
const AMPLIFYBACKEND_HOST: &str = "amplifybackend.us-east-1.amazonaws.com";

/// A SigV4 `Authorization` header whose credential scope names `service`, which is the
/// routing key the router claims a request by. The key id and signature are not
/// credentials: this harness turns request authentication off, so nothing is verified.
fn authorization(service: &str) -> String {
    authorization_in(service, "us-east-1")
}

/// [`authorization`] whose credential scope names `region`, which is how the emulator reads
/// WHICH Region's endpoint a client addressed it as.
fn authorization_in(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// AmplifyBackend and S3 mounted on ONE shared engine, the way the router mounts every
/// service: the bucket store S3 writes is the store AmplifyBackend reports.
fn router() -> MultiServiceRouter {
    router_on_shared_engine().0
}

/// The same pair of services, handing back the shared engine as well — what the one case
/// about a bucket ANOTHER service provisioned needs, since no S3 API creates such a
/// bucket.
fn router_on_shared_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    (router, engine)
}

/// `PUT /<bucket>` — S3's `CreateBucket`, with the location constraint (when any) in its
/// XML payload. Answers the HTTP status.
///
/// The request is addressed to the endpoint of the Region the constraint names, the way a
/// client that creates a bucket there addresses S3: an endpoint for one Region cannot place
/// a bucket in another, and an absent constraint names `us-east-1`.
async fn create_bucket(
    router: &MultiServiceRouter,
    bucket: &str,
    location_constraint: Option<&str>,
) -> u16 {
    let body = match location_constraint {
        None => String::new(),
        Some(constraint) => format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{constraint}</LocationConstraint>\
             </CreateBucketConfiguration>"
        ),
    };
    // The Region the constraint names, through the one reading S3 itself uses: absent means
    // `us-east-1` and the legacy `EU` means `eu-west-1`
    // (`CreateBucketConfiguration$LocationConstraint`).
    let region = service_com_amazonaws_s3_amazons3::state::bucket_region(location_constraint);
    let host = format!("s3.{region}.amazonaws.com");
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://{host}/{bucket}"))
        .header("host", &host)
        .header("authorization", authorization_in("s3", &region))
        .header("content-type", "application/xml")
        .body(Bytes::from(body))
        .expect("build the CreateBucket request");
    router.dispatch(request).await.status().as_u16()
}

/// `POST /s3Buckets` — AmplifyBackend's `ListS3Buckets`, carrying `next_token` when the
/// case passes one. Answers `(status, body)`.
async fn list_s3_buckets(
    router: &MultiServiceRouter,
    next_token: Option<&str>,
) -> (u16, Value) {
    let document = match next_token {
        Some(token) => json!({ "nextToken": token }),
        None => json!({}),
    };
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{AMPLIFYBACKEND_HOST}/s3Buckets"))
        .header("host", AMPLIFYBACKEND_HOST)
        .header("authorization", authorization("amplifybackend"))
        .header("content-type", "application/json")
        .body(Bytes::from(
            serde_json::to_vec(&document).expect("serialize the request body"),
        ))
        .expect("build the ListS3Buckets request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let body = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    (status, body)
}

/// Whole seconds since the epoch, the resolution a reported `creationDate` is bounded at.
fn now_seconds() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs(),
    )
    .expect("the epoch second fits in an i64")
}

/// A reported `creationDate` as whole seconds since the epoch. Parsed, not compared as
/// text: the rendering omits a zero fractional part, so the strings do not order.
fn reported_seconds(case: &str, bucket: &str, value: &Value) -> i64 {
    let text = value
        .get("creationDate")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: bucket {bucket:?} reports no creationDate, got {value:?}")
        });
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: bucket {bucket:?} reports creationDate {text:?}, \
                 which is not an RFC-3339 instant: {error}"
            )
        })
        .secs()
}

/// One case: the buckets to create, in the order the calls are made.
struct Case {
    name: &'static str,
    /// `(bucket name, CreateBucketConfiguration.LocationConstraint)`. The constraint
    /// values are members of S3's own `BucketLocationConstraint` enum (`EU` is its legacy
    /// value for `eu-west-1`); `None` leaves the bucket in the default Region.
    buckets: Vec<(String, Option<&'static str>)>,
}

/// The cases, differing along what the rule generalises over: how many buckets the
/// account holds, the form of their names, the Region each lives in, and the order they
/// were created in.
fn cases() -> Vec<Case> {
    // AWS's bucket naming rules bound a general purpose bucket name at 3 to 63
    // characters, so these are the shortest and the longest a client can create.
    let shortest = "abc".to_string();
    let longest = "b".repeat(63);
    vec![
        Case {
            name: "an account that holds no buckets",
            buckets: Vec::new(),
        },
        Case {
            name: "one bucket in the default Region",
            buckets: vec![("amplify-app-storage".to_string(), None)],
        },
        Case {
            name: "created out of name order",
            buckets: vec![
                ("zeta-storage".to_string(), None),
                ("alpha-storage".to_string(), None),
                ("mid-storage".to_string(), None),
            ],
        },
        Case {
            name: "buckets in three different Regions",
            buckets: vec![
                ("eu-storage".to_string(), Some("EU")),
                ("ap-storage".to_string(), Some("ap-southeast-2")),
                ("default-storage".to_string(), None),
            ],
        },
        Case {
            name: "the shortest and the longest legal bucket names",
            buckets: vec![(longest, None), (shortest, None)],
        },
        Case {
            name: "many buckets",
            buckets: (0..25)
                .map(|index| (format!("amplify-storage-{index:02}"), None))
                .collect(),
        },
    ]
}

/// Every bucket S3 holds is reported, in name order, with the instant it was created —
/// and nothing else is.
#[tokio::test]
async fn list_s3_buckets_reports_every_bucket_the_account_holds() {
    for case in cases() {
        let router = router();
        let created_from = now_seconds();
        for (bucket, location_constraint) in &case.buckets {
            let status = create_bucket(&router, bucket, *location_constraint).await;
            assert_eq!(
                status, 200,
                "case {:?}: creating bucket {bucket:?} must succeed, got HTTP {status}",
                case.name
            );
        }
        let created_until = now_seconds();

        let (status, body) = list_s3_buckets(&router, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: ListS3Buckets must answer 200, got HTTP {status} with {body:?}",
            case.name
        );

        let reported = body
            .get("buckets")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: ListS3Buckets must report a buckets list, got {body:?}",
                    case.name
                )
            });

        let reported_names: Vec<String> = reported
            .iter()
            .map(|bucket| {
                bucket
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let mut expected_names: Vec<String> =
            case.buckets.iter().map(|(name, _)| name.clone()).collect();
        expected_names.sort();
        assert_eq!(
            reported_names, expected_names,
            "case {:?}: expected the account's buckets in name order {expected_names:?}, \
             got {reported_names:?}",
            case.name
        );

        for bucket in reported {
            let name = bucket.get("name").and_then(Value::as_str).unwrap_or_default();
            let seconds = reported_seconds(case.name, name, bucket);
            assert!(
                (created_from..=created_until).contains(&seconds),
                "case {:?}: bucket {name:?} must report the instant it was created \
                 (between {created_from} and {created_until}), got {seconds}",
                case.name
            );
        }

        assert!(
            body.get("nextToken").is_none(),
            "case {:?}: NextToken is reserved and the answer is the whole account, \
             so none may be reported, got {body:?}",
            case.name
        );

        // Reporting, and a reserved member decides nothing: a second call — this one
        // carrying a NextToken — answers the identical document.
        let (repeat_status, repeat_body) = list_s3_buckets(&router, Some("reserved-token")).await;
        assert_eq!(
            (repeat_status, &repeat_body),
            (200, &body),
            "case {:?}: a repeated ListS3Buckets must answer identically, got HTTP \
             {repeat_status} with {repeat_body:?}",
            case.name
        );
    }
}

/// A bucket created AFTER a first `ListS3Buckets` is reported by the next one, and the
/// buckets already reported keep the creation instant they were reported with: the
/// operation reads S3's store rather than a snapshot of its own.
#[tokio::test]
async fn a_bucket_created_after_a_list_is_reported_by_the_next_one() {
    let router = router();
    let first = "amplify-storage-before";
    let second = "amplify-storage-after";

    assert_eq!(
        create_bucket(&router, first, None).await,
        200,
        "creating bucket {first:?} must succeed"
    );
    let (status, before) = list_s3_buckets(&router, None).await;
    assert_eq!(status, 200, "the first ListS3Buckets must answer 200");

    assert_eq!(
        create_bucket(&router, second, Some("us-west-2")).await,
        200,
        "creating bucket {second:?} must succeed"
    );
    let (status, after) = list_s3_buckets(&router, None).await;
    assert_eq!(status, 200, "the second ListS3Buckets must answer 200");

    let names: Vec<&str> = after
        .get("buckets")
        .and_then(Value::as_array)
        .map(|buckets| {
            buckets
                .iter()
                .filter_map(|bucket| bucket.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![second, first],
        "both buckets must be reported in name order, got {names:?}"
    );

    let entry_for = |body: &Value, bucket: &str| -> Value {
        body.get("buckets")
            .and_then(Value::as_array)
            .and_then(|buckets| {
                buckets
                    .iter()
                    .find(|entry| entry.get("name").and_then(Value::as_str) == Some(bucket))
                    .cloned()
            })
            .unwrap_or(Value::Null)
    };
    assert_eq!(
        entry_for(&before, first),
        entry_for(&after, first),
        "bucket {first:?} must keep the creation date it was first reported with, \
         got {:?} then {:?}",
        entry_for(&before, first),
        entry_for(&after, first)
    );
}

/// A bucket ANOTHER emulated service provisioned as the object store of one of its own
/// resources is not a bucket of this account, so it is not reported — while every bucket
/// created over S3's own API still is.
///
/// No S3 API creates such a bucket (the owning service's control plane does, through the
/// S3 store's `provision_bucket` seam), so this one case reaches that seam directly; the
/// account's own buckets are still created over the wire.
#[tokio::test]
async fn a_bucket_another_service_provisioned_is_not_a_bucket_of_this_account() {
    let (router, engine) = router_on_shared_engine();

    // (case, the bucket name, the service that provisioned it — `None` for one created
    // over S3's own API, so it belongs to the account)
    let rows: &[(&str, &str, Option<&str>)] = &[
        ("created over the S3 API", "amplify-app-storage", None),
        (
            "provisioned by another service",
            "lightsail-object-store",
            Some("lightsail"),
        ),
        ("another bucket of the account", "amplify-logs", None),
        (
            "provisioned by a second service",
            "another-managed-store",
            Some("someotherservice"),
        ),
    ];

    for (case, bucket, provisioned_by) in rows {
        match provisioned_by {
            None => assert_eq!(
                create_bucket(&router, bucket, None).await,
                200,
                "case {case:?}: creating bucket {bucket:?} must succeed"
            ),
            Some(service) => {
                let state = service_com_amazonaws_s3_amazons3::state::service_state(&*engine)
                    .expect("the S3 store is reachable from the shared engine");
                let outcome = state.lock().provision_bucket(bucket, "us-east-1", service);
                assert_eq!(
                    outcome,
                    service_com_amazonaws_s3_amazons3::state::BucketCreation::Created,
                    "case {case:?}: provisioning bucket {bucket:?} must record it"
                );
            }
        }
    }

    let (status, body) = list_s3_buckets(&router, None).await;
    assert_eq!(
        status, 200,
        "ListS3Buckets must answer 200, got HTTP {status} with {body:?}"
    );
    let reported: Vec<&str> = body
        .get("buckets")
        .and_then(Value::as_array)
        .map(|buckets| {
            buckets
                .iter()
                .filter_map(|bucket| bucket.get("name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    let mut expected: Vec<&str> = rows
        .iter()
        .filter(|(_, _, provisioned_by)| provisioned_by.is_none())
        .map(|(_, bucket, _)| *bucket)
        .collect();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "expected only the account's own buckets {expected:?}, got {reported:?}"
    );

    // The provisioned buckets ARE in the store — the filter is what leaves them out of
    // this account's listing, not their absence.
    let state = service_com_amazonaws_s3_amazons3::state::service_state(&*engine)
        .expect("the S3 store is reachable from the shared engine");
    let store = state.lock();
    for (case, bucket, provisioned_by) in rows {
        let held = store
            .bucket(bucket)
            .unwrap_or_else(|| panic!("case {case:?}: the store must hold bucket {bucket:?}"));
        assert_eq!(
            held.provisioned_by(),
            *provisioned_by,
            "case {case:?}: expected the provisioner {provisioned_by:?}, got {:?}",
            held.provisioned_by()
        );
    }
}
