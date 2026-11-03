// SPDX-License-Identifier: CC0-1.0

//! Deleting a Lightsail bucket a CDN distribution pulls its content from, driven
//! over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a Region.
//!
//! The rule under test: a `DeleteBucket` that does not force the delete is
//! refused, with the service's `InvalidInputException`, while the bucket still
//! satisfies ANY of the conditions the modelled `DeleteBucketRequest$forceDelete`
//! documents a force delete as required for — and being the origin a distribution
//! of the account pulls from is one of them, counted from the distribution
//! records that name the bucket, so the condition appears when such a
//! distribution is created and goes away when it is deleted. The refusal changes
//! nothing; `forceDelete` goes through and, as that member documents, "might
//! impact other resources that rely on the bucket, such as instances,
//! distributions, or software that use the issued access keys" — the distribution
//! itself is not deleted with its origin.
//!
//! So the cases are a TABLE: rows differ along how many distributions pull from
//! the bucket (none, one, two), the three forms `forceDelete` takes (absent,
//! false, true), what else the bucket holds at the same time (an access key, so
//! two conditions are reported together), what the distributions beside it pull
//! from instead (another bucket, an instance — the other kinds of resource an
//! origin may be), whether the distribution that named the bucket has since been
//! deleted, and the Region the bucket under delete lives in.
//!
//! `bucket_object_data_plane.rs` owns the rows for the objects and access-key
//! conditions of the same rule; these rows are the origin condition it does not
//! reach.

use serde_json::{json, Value};

use crate::disk_attachment::create_running_instance;
use crate::instance_lifecycle::{error_code, Emulator};

/// The one Region the distribution APIs are served in, from the modelled
/// `InvalidInputException` documentation ("Domain and distribution APIs are only
/// available in the N. Virginia (us-east-1) Amazon Web Services Region"), which
/// is also the Region an unscoped call is made in.
const DISTRIBUTION_REGION: &str = "us-east-1";

/// A second Region, so a case can delete a bucket that does not live where the
/// distribution APIs are served.
const WEST_REGION: &str = "us-west-2";

/// The Availability Zone an origin instance is created in: the first zone of the
/// distribution Region.
const ORIGIN_ZONE: &str = "us-east-1a";

/// The bucket plan every bucket here is created on (`GetBucketBundles`), and the
/// CDN plan every distribution runs on (`GetDistributionBundles`): neither is
/// what any case is about.
const BUCKET_BUNDLE: &str = "small_1_0";
/// See [`BUCKET_BUNDLE`].
const DISTRIBUTION_BUNDLE: &str = "small_1_0";

/// The wire error code (awsJson `__type`) of a refusal of a request the model
/// admits but the service does not serve.
const INVALID_INPUT: &str = "InvalidInputException";

/// The wire error code of Lightsail's rejection of a name that addresses
/// nothing, and the `code` its body carries.
const NOT_FOUND: &str = "NotFoundException";
/// See [`NOT_FOUND`].
const DOES_NOT_EXIST: &str = "DoesNotExist";

/// One distribution a case stands up: its name, the origin resource it pulls
/// from and the Region that origin is named in.
type DistributionFixture = (&'static str, &'static str, &'static str);

/// One delete case: what the account holds when the delete is made, whether the
/// request forces it, and what must happen.
struct Row {
    case: &'static str,
    /// Every bucket this case stands up, as `(Region, name)`. A bucket name
    /// addresses one bucket across the whole object namespace, so two Regions'
    /// buckets differ in name.
    buckets: &'static [(&'static str, &'static str)],
    /// The instances this case stands up to serve as distribution origins, in
    /// the distribution Region.
    instances: &'static [&'static str],
    /// The distributions this case creates.
    distributions: &'static [DistributionFixture],
    /// The distributions deleted again before the `DeleteBucket` call: an origin
    /// condition a distribution put on a bucket is gone once the distribution is.
    deleted_distributions: &'static [&'static str],
    /// How many access keys the bucket under delete holds, so a row can hold two
    /// of the documented conditions at once.
    access_keys: usize,
    /// The bucket the delete names, as `(Region, name)`.
    target: (&'static str, &'static str),
    /// The `forceDelete` the request carries, absent when `None`.
    force_delete: Option<bool>,
    /// The HTTP status the delete must answer.
    status: u16,
    /// The error code it must carry, or `None` for a delete that goes through.
    error_code: Option<&'static str>,
    /// Fragments the refusal's message must carry: the service's own sentence
    /// for a bucket a force delete is required for, which names the bucket and
    /// the condition class and nothing else — WHICH conditions block this
    /// delete is a diagnostic of the emulator, logged rather than returned.
    message_fragments: &'static [&'static str],
}

const ORIGIN_BUCKET: &str = "cdn-origin-bucket";
const OTHER_BUCKET: &str = "other-origin-bucket";
const WEST_BUCKET: &str = "west-origin-bucket";
const WEST_FREE_BUCKET: &str = "west-unused-bucket";
const ORIGIN_INSTANCE: &str = "origin-instance";

const DELETE_ROWS: &[Row] = &[
    Row {
        case: "a bucket no distribution pulls from, flag absent",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "the origin of one distribution, flag absent",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[ORIGIN_BUCKET, "in use"],
    },
    Row {
        case: "the origin of one distribution, forceDelete false",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: Some(false),
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[ORIGIN_BUCKET, "in use"],
    },
    Row {
        case: "the origin of two distributions, flag absent",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[
            ("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_BUCKET, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[ORIGIN_BUCKET, "in use"],
    },
    Row {
        case: "the origin of one distribution, forceDelete true",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: Some(true),
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "the origin of two distributions, forceDelete true",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[
            ("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_BUCKET, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: Some(true),
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "the origin of one distribution and holding one access key, flag absent",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 1,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[ORIGIN_BUCKET, "in use"],
    },
    Row {
        case: "a bucket beside a distribution that pulls from another bucket",
        buckets: &[
            (DISTRIBUTION_REGION, ORIGIN_BUCKET),
            (DISTRIBUTION_REGION, OTHER_BUCKET),
        ],
        instances: &[],
        distributions: &[("cdn-one", OTHER_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "a bucket beside a distribution that pulls from an instance",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[ORIGIN_INSTANCE],
        distributions: &[("cdn-one", ORIGIN_INSTANCE, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "the origin of one of two distributions, flag absent",
        buckets: &[
            (DISTRIBUTION_REGION, ORIGIN_BUCKET),
            (DISTRIBUTION_REGION, OTHER_BUCKET),
        ],
        instances: &[],
        distributions: &[
            ("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION),
            ("cdn-two", OTHER_BUCKET, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[ORIGIN_BUCKET, "in use"],
    },
    Row {
        case: "the origin of a distribution that has since been deleted, flag absent",
        buckets: &[(DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &["cdn-one"],
        access_keys: 0,
        target: (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "a bucket in another Region that is the origin of a distribution, flag absent",
        buckets: &[(WEST_REGION, WEST_BUCKET)],
        instances: &[],
        distributions: &[("cdn-west", WEST_BUCKET, WEST_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (WEST_REGION, WEST_BUCKET),
        force_delete: None,
        status: 400,
        error_code: Some(INVALID_INPUT),
        message_fragments: &[WEST_BUCKET, "in use"],
    },
    Row {
        case: "a bucket in another Region no distribution pulls from, flag absent",
        buckets: &[
            (WEST_REGION, WEST_FREE_BUCKET),
            (DISTRIBUTION_REGION, ORIGIN_BUCKET),
        ],
        instances: &[],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (WEST_REGION, WEST_FREE_BUCKET),
        force_delete: None,
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
    Row {
        case: "a bucket in another Region that is the origin of a distribution, forceDelete true",
        buckets: &[(WEST_REGION, WEST_BUCKET)],
        instances: &[],
        distributions: &[("cdn-west", WEST_BUCKET, WEST_REGION)],
        deleted_distributions: &[],
        access_keys: 0,
        target: (WEST_REGION, WEST_BUCKET),
        force_delete: Some(true),
        status: 200,
        error_code: None,
        message_fragments: &[],
    },
];

/// Create the bucket `name` in `region`.
async fn create_bucket(emulator: &Emulator, region: &str, name: &str) {
    emulator
        .ok_at(
            None,
            Some(region),
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUCKET_BUNDLE}),
        )
        .await;
}

/// Create the distribution `name`, pulling from the origin `origin` named in
/// `origin_region`.
async fn create_distribution(emulator: &Emulator, name: &str, origin: &str, origin_region: &str) {
    emulator
        .ok(
            "CreateDistribution",
            json!({
                "distributionName": name,
                "origin": {
                    "name": origin,
                    "regionName": origin_region,
                    "protocolPolicy": "http-only",
                },
                "defaultCacheBehavior": {"behavior": "cache"},
                "bundleId": DISTRIBUTION_BUNDLE,
            }),
        )
        .await;
}

/// The distribution `name`, as `GetDistributions` reports it, or `Value::Null`
/// when this account holds none of that name.
async fn distribution(emulator: &Emulator, name: &str) -> Value {
    let (status, body) = emulator
        .call("GetDistributions", json!({"distributionName": name}))
        .await;
    if status != 200 {
        return Value::Null;
    }
    body["distributions"]
        .as_array()
        .and_then(|reported| reported.first())
        .cloned()
        .unwrap_or(Value::Null)
}

/// Whether `region` holds a bucket called `name`.
async fn bucket_exists(emulator: &Emulator, region: &str, name: &str) -> bool {
    let (status, body) = emulator
        .call_in(Some(region), "GetBuckets", json!({"bucketName": name}))
        .await;
    status == 200
        && body["buckets"]
            .as_array()
            .and_then(|buckets| buckets.first())
            .and_then(|bucket| bucket["name"].as_str())
            == Some(name)
}

/// A `DeleteBucket` without `forceDelete` is refused while the bucket is the
/// origin a distribution of the account pulls from, exactly as it is for the
/// other conditions that member documents; the refusal leaves everything as it
/// was, and a forced delete goes through and leaves the distribution in place.
#[tokio::test]
async fn delete_bucket_gates_on_the_distributions_that_pull_from_it() {
    for row in DELETE_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let (target_region, target) = row.target;

        for (region, name) in row.buckets {
            create_bucket(&emulator, region, name).await;
        }
        for name in row.instances {
            create_running_instance(&emulator, name, ORIGIN_ZONE).await;
        }
        for (name, origin, origin_region) in row.distributions {
            create_distribution(&emulator, name, origin, origin_region).await;
        }
        for name in row.deleted_distributions {
            emulator
                .ok("DeleteDistribution", json!({"distributionName": name}))
                .await;
        }
        for index in 0..row.access_keys {
            let minted = emulator
                .ok_at(
                    None,
                    Some(target_region),
                    "CreateBucketAccessKey",
                    json!({"bucketName": target}),
                )
                .await;
            assert!(
                minted["accessKey"]["accessKeyId"].as_str().is_some(),
                "case {case:?}: access key {index} must be minted, got {minted:?}"
            );
        }

        let mut request = json!({"bucketName": target});
        if let Some(force_delete) = row.force_delete {
            request["forceDelete"] = json!(force_delete);
        }
        let (status, body) = emulator
            .call_in(Some(target_region), "DeleteBucket", request)
            .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} with {body:?}",
            row.status
        );

        match row.error_code {
            Some(expected) => {
                let code = error_code(&body);
                assert_eq!(
                    code, expected,
                    "case {case:?}: expected {expected}, got {code} with {body:?}"
                );
                // The refusal names the bucket and the condition class, and
                // carries nothing else: the per-condition detail is logged, not
                // put in the modelled message.
                let message = body["message"].as_str().unwrap_or_default();
                for fragment in row.message_fragments {
                    assert!(
                        message.contains(fragment),
                        "case {case:?}: expected the refusal to carry {fragment:?}, \
                         got {message:?}"
                    );
                }
                // Nothing moved: the bucket is still there, and so is every
                // distribution that pulls from it.
                assert!(
                    bucket_exists(&emulator, target_region, target).await,
                    "case {case:?}: the refused delete must leave the bucket {target:?} in \
                     {target_region}"
                );
            }
            None => {
                let operations = body["operations"].as_array().unwrap_or_else(|| {
                    panic!("case {case:?}: a delete must answer its operations, got {body:?}")
                });
                assert_eq!(
                    operations.len(),
                    1,
                    "case {case:?}: expected one operation, got {operations:?}"
                );
                assert_eq!(
                    operations[0]["operationType"].as_str(),
                    Some("DeleteBucket"),
                    "case {case:?}: expected a DeleteBucket operation, got {:?}",
                    operations[0]
                );
                assert_eq!(
                    operations[0]["resourceName"].as_str(),
                    Some(target),
                    "case {case:?}: the operation must name the bucket, got {:?}",
                    operations[0]
                );
                // The bucket is gone: its Region's reader no longer holds the name.
                let (missing_status, missing) = emulator
                    .call_in(Some(target_region), "GetBuckets", json!({"bucketName": target}))
                    .await;
                assert_eq!(
                    missing_status, 400,
                    "case {case:?}: the deleted bucket must be gone, got {missing_status} \
                     with {missing:?}"
                );
                assert_eq!(
                    error_code(&missing),
                    NOT_FOUND,
                    "case {case:?}: expected {NOT_FOUND} for the deleted bucket, got {missing:?}"
                );
                assert_eq!(
                    missing["code"].as_str(),
                    Some(DOES_NOT_EXIST),
                    "case {case:?}: expected the body code {DOES_NOT_EXIST}, got {missing:?}"
                );
            }
        }

        // Whichever way the delete went, it deleted no distribution: a forced
        // delete "might impact other resources that rely on the bucket, such as
        // instances, distributions", and the ones it impacts stay as they are.
        for (name, origin, origin_region) in row.distributions {
            if row.deleted_distributions.contains(name) {
                continue;
            }
            let reported = distribution(&emulator, name).await;
            assert_eq!(
                reported["name"].as_str(),
                Some(*name),
                "case {case:?}: the distribution {name:?} must survive, got {reported:?}"
            );
            assert_eq!(
                reported["origin"]["name"].as_str(),
                Some(*origin),
                "case {case:?}: the distribution {name:?} must still report its origin, \
                 got {reported:?}"
            );
            assert_eq!(
                reported["origin"]["regionName"].as_str(),
                Some(*origin_region),
                "case {case:?}: the distribution {name:?} must still report its origin \
                 Region, got {reported:?}"
            );
        }
    }
}
