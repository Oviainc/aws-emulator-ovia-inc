// SPDX-License-Identifier: CC0-1.0

//! Deleting the Lightsail resource a CDN distribution pulls its content from,
//! driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness, the instance stand-up and the load balancer stand-up
//! are the ones this test target already exposes.
//!
//! The rule under test: a distribution pulls its content from ONE origin
//! resource of this account — "an origin can be a Lightsail instance, bucket,
//! container service, or load balancer" (the modelled `InputOrigin`) — so a
//! resource a distribution of the account names is IN USE by it, and a delete
//! that would leave the distribution pulling from a resource the account no
//! longer holds is refused with the service's `InvalidInputException`, naming the
//! resource and how many distributions pull from it. The condition is the one the
//! modelled `DeleteBucketRequest$forceDelete` documents ("You must force delete
//! the bucket if it has one of the following conditions: The bucket is the origin
//! of a distribution"), so it holds for all four kinds of origin resource, and a
//! delete that offers a force — `DeleteBucket` is the only one that does — is
//! what overrides it. The refusal changes nothing, and a delete the condition
//! does not block deletes exactly the one resource it names.
//!
//! So the cases are a TABLE: rows differ along which of the four kinds of origin
//! resource the delete names, how many distributions pull from it (none, one,
//! two), whether a distribution that named it has since been deleted, whether the
//! distribution beside it pulls from a different resource, the Region the
//! resource lives in, whether a same-named resource in another Region is the one
//! being pulled from, and — for the one delete that has the flag — the three
//! forms `forceDelete` takes.
//!
//! `bucket_distribution_origin_delete.rs` owns the rows that cross this condition
//! with the OTHER conditions a bucket's force delete is documented for; these
//! rows are the kinds of origin resource it does not reach.

use serde_json::{json, Value};

use crate::disk_attachment::create_running_instance;
use crate::instance_lifecycle::{error_code, Emulator};
use crate::load_balancer_instance_attachment::create_load_balancer;

/// The one Region the distribution APIs are served in, from the modelled
/// `InvalidInputException` documentation ("Domain and distribution APIs are only
/// available in the N. Virginia (us-east-1) Amazon Web Services Region"), which
/// is also the Region an unscoped call is made in.
const DISTRIBUTION_REGION: &str = "us-east-1";

/// A second Region, so a case can delete an origin resource that does not live
/// where the distribution APIs are served: a distribution "can reference an
/// origin in any Amazon Web Services Region" (the modelled
/// `LightsailDistribution$location`).
const WEST_REGION: &str = "us-west-2";

/// The bucket plan every bucket here is created on (`GetBucketBundles`), the CDN
/// plan every distribution runs on (`GetDistributionBundles`), the container
/// service plan and scale and the load balancer's instance port: none of them is
/// what any case is about.
const BUCKET_BUNDLE: &str = "small_1_0";
/// See [`BUCKET_BUNDLE`].
const DISTRIBUTION_BUNDLE: &str = "small_1_0";
/// See [`BUCKET_BUNDLE`].
const CONTAINER_POWER: &str = "nano";
/// See [`BUCKET_BUNDLE`].
const CONTAINER_SCALE: i64 = 1;
/// See [`BUCKET_BUNDLE`].
const INSTANCE_PORT: i64 = 80;

/// The wire error code (awsJson `__type`) of a refusal of a request the model
/// admits but the service does not serve in the state the account is in.
const INVALID_INPUT: &str = "InvalidInputException";

/// Which kind of resource a distribution's origin is, from the modelled
/// `InputOrigin` documentation: "An origin can be a Lightsail instance, bucket,
/// container service, or load balancer."
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Instance,
    Bucket,
    ContainerService,
    LoadBalancer,
}

impl Kind {
    /// The modelled `ResourceType` member this kind of resource reports, which a
    /// refusal naming it uses.
    fn resource_type(self) -> &'static str {
        match self {
            Kind::Instance => "Instance",
            Kind::Bucket => "Bucket",
            Kind::ContainerService => "ContainerService",
            Kind::LoadBalancer => "LoadBalancer",
        }
    }

    /// The HTTP status this kind's delete answers when it goes through: the
    /// modelled success status of that operation.
    fn delete_success_status(self) -> u16 {
        match self {
            // `DeleteContainerServiceResult` declares no member at all.
            Kind::ContainerService => 204,
            _ => 200,
        }
    }
}

/// One resource a case stands up, as `(kind, Region, name)`.
type Fixture = (Kind, &'static str, &'static str);

/// One distribution a case stands up: its name, the origin resource it pulls
/// from and the Region that origin is named in.
type DistributionFixture = (&'static str, &'static str, &'static str);

/// One delete case: what the account holds when the delete is made, whether the
/// request forces it, and what must happen.
struct Row {
    case: &'static str,
    /// Every resource this case stands up, the one under delete included.
    resources: &'static [Fixture],
    /// The distributions this case creates, in order.
    distributions: &'static [DistributionFixture],
    /// The distributions deleted again before the delete under test: the
    /// condition a distribution put on its origin is gone once the distribution
    /// is.
    deleted_distributions: &'static [&'static str],
    /// The resource the delete names.
    target: Fixture,
    /// The `forceDelete` the request carries, for the one delete that has the
    /// flag; absent when `None`.
    force_delete: Option<bool>,
    /// How many distributions must block the delete: 0 for a delete that goes
    /// through, which is also what the refusal's message must report.
    blocking_distributions: usize,
}

const ORIGIN_INSTANCE: &str = "origin-instance";
const OTHER_INSTANCE: &str = "other-instance";
const WEST_INSTANCE: &str = "west-origin-instance";
const ORIGIN_BUCKET: &str = "cdn-origin-bucket";
const ORIGIN_SERVICE: &str = "origin-svc";
const WEST_SERVICE: &str = "west-origin-svc";
const ORIGIN_BALANCER: &str = "origin-balancer";
const WEST_BALANCER: &str = "west-origin-balancer";

const ROWS: &[Row] = &[
    // --- the instance origin -------------------------------------------------
    Row {
        case: "an instance no distribution pulls from",
        resources: &[(Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE)],
        distributions: &[],
        deleted_distributions: &[],
        target: (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "an instance one distribution pulls from",
        resources: &[(Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE)],
        distributions: &[("cdn-one", ORIGIN_INSTANCE, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        force_delete: None,
        blocking_distributions: 1,
    },
    Row {
        case: "an instance two distributions pull from",
        resources: &[(Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE)],
        distributions: &[
            ("cdn-one", ORIGIN_INSTANCE, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_INSTANCE, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        target: (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        force_delete: None,
        blocking_distributions: 2,
    },
    Row {
        case: "an instance whose distribution has since been deleted",
        resources: &[(Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE)],
        distributions: &[("cdn-one", ORIGIN_INSTANCE, DISTRIBUTION_REGION)],
        deleted_distributions: &["cdn-one"],
        target: (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "an instance beside a distribution that pulls from another instance",
        resources: &[
            (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
            (Kind::Instance, DISTRIBUTION_REGION, OTHER_INSTANCE),
        ],
        distributions: &[("cdn-one", OTHER_INSTANCE, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "an instance of another Region one distribution pulls from",
        resources: &[(Kind::Instance, WEST_REGION, WEST_INSTANCE)],
        distributions: &[("cdn-west", WEST_INSTANCE, WEST_REGION)],
        deleted_distributions: &[],
        target: (Kind::Instance, WEST_REGION, WEST_INSTANCE),
        force_delete: None,
        blocking_distributions: 1,
    },
    // --- the container service origin ---------------------------------------
    Row {
        case: "a container service no distribution pulls from",
        resources: &[(Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE)],
        distributions: &[],
        deleted_distributions: &[],
        target: (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "a container service one distribution pulls from",
        resources: &[(Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE)],
        distributions: &[("cdn-one", ORIGIN_SERVICE, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
        force_delete: None,
        blocking_distributions: 1,
    },
    Row {
        case: "a container service two distributions pull from",
        resources: &[(Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE)],
        distributions: &[
            ("cdn-one", ORIGIN_SERVICE, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_SERVICE, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        target: (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
        force_delete: None,
        blocking_distributions: 2,
    },
    Row {
        case: "a container service whose distribution has since been deleted",
        resources: &[(Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE)],
        distributions: &[("cdn-one", ORIGIN_SERVICE, DISTRIBUTION_REGION)],
        deleted_distributions: &["cdn-one"],
        target: (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "a container service of another Region one distribution pulls from",
        resources: &[(Kind::ContainerService, WEST_REGION, WEST_SERVICE)],
        distributions: &[("cdn-west", WEST_SERVICE, WEST_REGION)],
        deleted_distributions: &[],
        target: (Kind::ContainerService, WEST_REGION, WEST_SERVICE),
        force_delete: None,
        blocking_distributions: 1,
    },
    Row {
        case: "a container service whose same name in another Region is the origin",
        resources: &[
            (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
            (Kind::ContainerService, WEST_REGION, ORIGIN_SERVICE),
        ],
        distributions: &[("cdn-one", ORIGIN_SERVICE, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::ContainerService, WEST_REGION, ORIGIN_SERVICE),
        force_delete: None,
        blocking_distributions: 0,
    },
    // --- the load balancer origin -------------------------------------------
    Row {
        case: "a load balancer no distribution pulls from",
        resources: &[(Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER)],
        distributions: &[],
        deleted_distributions: &[],
        target: (Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "a load balancer one distribution pulls from",
        resources: &[(Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER)],
        distributions: &[("cdn-one", ORIGIN_BALANCER, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER),
        force_delete: None,
        blocking_distributions: 1,
    },
    Row {
        case: "a load balancer two distributions pull from",
        resources: &[(Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER)],
        distributions: &[
            ("cdn-one", ORIGIN_BALANCER, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_BALANCER, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        target: (Kind::LoadBalancer, DISTRIBUTION_REGION, ORIGIN_BALANCER),
        force_delete: None,
        blocking_distributions: 2,
    },
    Row {
        case: "a load balancer of another Region one distribution pulls from",
        resources: &[(Kind::LoadBalancer, WEST_REGION, WEST_BALANCER)],
        distributions: &[("cdn-west", WEST_BALANCER, WEST_REGION)],
        deleted_distributions: &[],
        target: (Kind::LoadBalancer, WEST_REGION, WEST_BALANCER),
        force_delete: None,
        blocking_distributions: 1,
    },
    // --- the bucket origin, the one delete that offers a force ---------------
    Row {
        case: "a bucket no distribution pulls from, flag absent",
        resources: &[(Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        distributions: &[],
        deleted_distributions: &[],
        target: (Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        blocking_distributions: 0,
    },
    Row {
        case: "a bucket one distribution pulls from, flag absent",
        resources: &[(Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: None,
        blocking_distributions: 1,
    },
    Row {
        case: "a bucket one distribution pulls from, forceDelete false",
        resources: &[(Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        distributions: &[("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION)],
        deleted_distributions: &[],
        target: (Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: Some(false),
        blocking_distributions: 1,
    },
    Row {
        case: "a bucket two distributions pull from, forceDelete true",
        resources: &[(Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET)],
        distributions: &[
            ("cdn-one", ORIGIN_BUCKET, DISTRIBUTION_REGION),
            ("cdn-two", ORIGIN_BUCKET, DISTRIBUTION_REGION),
        ],
        deleted_distributions: &[],
        target: (Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET),
        force_delete: Some(true),
        blocking_distributions: 0,
    },
];

/// Stand up the resource `fixture` describes.
async fn create_resource(emulator: &Emulator, fixture: Fixture) {
    let (kind, region, name) = fixture;
    match kind {
        // An instance's Availability Zone is what puts it in a Region, which is
        // the Region its name is resolved as an origin in.
        Kind::Instance => create_running_instance(emulator, name, &format!("{region}a")).await,
        Kind::Bucket => {
            emulator
                .ok_at(
                    None,
                    Some(region),
                    "CreateBucket",
                    json!({"bucketName": name, "bundleId": BUCKET_BUNDLE}),
                )
                .await;
        }
        Kind::ContainerService => {
            // `CreateContainerService` answers 201, its modelled success status.
            let (status, body) = emulator
                .call_in(
                    Some(region),
                    "CreateContainerService",
                    json!({
                        "serviceName": name,
                        "power": CONTAINER_POWER,
                        "scale": CONTAINER_SCALE,
                    }),
                )
                .await;
            assert_eq!(
                status, 201,
                "fixture {name:?} in {region}: CreateContainerService must succeed, got \
                 {status} with {body:?}"
            );
        }
        Kind::LoadBalancer => create_load_balancer(emulator, region, name, INSTANCE_PORT).await,
    }
}

/// Delete the resource `fixture` describes, carrying `force_delete` where the
/// operation has that flag, and answer the HTTP status and parsed body.
async fn delete_resource(
    emulator: &Emulator,
    fixture: Fixture,
    force_delete: Option<bool>,
) -> (u16, Value) {
    let (kind, region, name) = fixture;
    let (operation, mut request) = match kind {
        Kind::Instance => ("DeleteInstance", json!({"instanceName": name})),
        Kind::Bucket => ("DeleteBucket", json!({"bucketName": name})),
        Kind::ContainerService => ("DeleteContainerService", json!({"serviceName": name})),
        Kind::LoadBalancer => ("DeleteLoadBalancer", json!({"loadBalancerName": name})),
    };
    if let Some(force_delete) = force_delete {
        assert_eq!(
            kind,
            Kind::Bucket,
            "fixture {name:?}: only DeleteBucket models a forceDelete"
        );
        request["forceDelete"] = json!(force_delete);
    }
    emulator.call_in(Some(region), operation, request).await
}

/// Whether the account still holds the resource `fixture` describes, read back
/// through that kind's own reporting operation.
async fn resource_exists(emulator: &Emulator, fixture: Fixture) -> bool {
    let (kind, region, name) = fixture;
    let (operation, request, collection) = match kind {
        Kind::Instance => ("GetInstance", json!({"instanceName": name}), None),
        Kind::Bucket => (
            "GetBuckets",
            json!({"bucketName": name}),
            Some(("buckets", "name")),
        ),
        Kind::ContainerService => (
            "GetContainerServices",
            json!({"serviceName": name}),
            Some(("containerServices", "containerServiceName")),
        ),
        Kind::LoadBalancer => ("GetLoadBalancer", json!({"loadBalancerName": name}), None),
    };
    let (status, body) = emulator.call_in(Some(region), operation, request).await;
    if status != 200 {
        return false;
    }
    match collection {
        Some((member, name_member)) => {
            body[member]
                .as_array()
                .and_then(|reported| reported.first())
                .and_then(|reported| reported[name_member].as_str())
                == Some(name)
        }
        None => !body.is_null(),
    }
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

/// How a refusal words the condition, for `count` distributions: the wording the
/// crate's one owner of it produces (`crate::state::origin_of_distributions_condition`).
fn origin_condition(count: usize) -> String {
    format!(
        "it is the origin of {count} distribution{}",
        if count == 1 { "" } else { "s" }
    )
}

/// A delete of the resource a distribution of the account pulls its content from
/// is refused while that reference exists — whichever of the four kinds of origin
/// resource it names — unless the delete offers a force and the request sets it;
/// the refusal changes nothing, and a delete the condition does not block removes
/// exactly the resource it names and leaves every distribution in place.
#[tokio::test]
async fn deleting_a_distributions_origin_resource_is_refused_while_it_is_pulled_from() {
    for row in ROWS {
        let case = row.case;
        let emulator = Emulator::new();

        for fixture in row.resources {
            create_resource(&emulator, *fixture).await;
        }
        for (name, origin, origin_region) in row.distributions {
            create_distribution(&emulator, name, origin, origin_region).await;
        }
        for name in row.deleted_distributions {
            emulator
                .ok("DeleteDistribution", json!({"distributionName": name}))
                .await;
        }

        let (_, _, target_name) = row.target;
        let (status, body) = delete_resource(&emulator, row.target, row.force_delete).await;

        if row.blocking_distributions > 0 {
            assert_eq!(
                status, 400,
                "case {case:?}: expected the delete refused with HTTP 400, got {status} \
                 with {body:?}"
            );
            let code = error_code(&body);
            assert_eq!(
                code, INVALID_INPUT,
                "case {case:?}: expected {INVALID_INPUT}, got {code} with {body:?}"
            );
            // The refusal names the resource and, for the deletes that word the
            // condition themselves, how many distributions pull from it.
            // `DeleteBucket` answers the service's own sentence alone — the
            // per-condition detail is logged, not returned — so its rows assert
            // the headline only.
            let message = body["message"].as_str().unwrap_or_default();
            let condition = origin_condition(row.blocking_distributions);
            let mut fragments = vec![row.target.0.resource_type(), target_name, "is in use"];
            if row.target.0 != Kind::Bucket {
                fragments.push(condition.as_str());
            }
            for fragment in fragments {
                assert!(
                    message.contains(fragment),
                    "case {case:?}: expected the refusal to carry {fragment:?}, got {message:?}"
                );
            }
            // Nothing moved: the refused delete left the resource where it was.
            assert!(
                resource_exists(&emulator, row.target).await,
                "case {case:?}: the refused delete must leave {target_name:?} in place"
            );
        } else {
            assert_eq!(
                status,
                row.target.0.delete_success_status(),
                "case {case:?}: expected HTTP {}, got {status} with {body:?}",
                row.target.0.delete_success_status()
            );
            // The resource is gone: its own reader no longer holds the name.
            assert!(
                !resource_exists(&emulator, row.target).await,
                "case {case:?}: the delete must remove {target_name:?}"
            );
        }

        // Whichever way the delete went, it deleted no distribution, and every
        // surviving distribution still reports the origin it was created with.
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
                (
                    reported["origin"]["name"].as_str(),
                    reported["origin"]["regionName"].as_str()
                ),
                (Some(*origin), Some(*origin_region)),
                "case {case:?}: the distribution {name:?} must still report the origin it was \
                 created with, got {reported:?}"
            );
        }

        // Every OTHER resource the case stood up survives a delete of this one:
        // the condition is about the resource the delete names, not about its
        // neighbours or its same-named sibling in another Region.
        for fixture in row.resources {
            if *fixture == row.target {
                continue;
            }
            let (_, region, name) = *fixture;
            assert!(
                resource_exists(&emulator, *fixture).await,
                "case {case:?}: {name:?} in {region} must survive the delete of \
                 {target_name:?}"
            );
        }
    }
}

/// The resource the delete was refused for can be deleted once the distributions
/// that pulled from it are gone, and the delete that then goes through is the
/// same call that was refused a moment earlier — so the refusal is a condition on
/// the account's state, not on the request.
///
/// One row per kind of origin resource, so the release is pinned for all four.
#[tokio::test]
async fn deleting_the_origin_resource_succeeds_once_no_distribution_pulls_from_it() {
    const CASES: &[Fixture] = &[
        (Kind::Instance, DISTRIBUTION_REGION, ORIGIN_INSTANCE),
        (Kind::Bucket, DISTRIBUTION_REGION, ORIGIN_BUCKET),
        (Kind::ContainerService, DISTRIBUTION_REGION, ORIGIN_SERVICE),
        (Kind::LoadBalancer, WEST_REGION, WEST_BALANCER),
    ];

    for fixture in CASES {
        let (kind, region, name) = *fixture;
        let case = kind.resource_type();
        let emulator = Emulator::new();
        create_resource(&emulator, *fixture).await;
        create_distribution(&emulator, "cdn-one", name, region).await;

        let (refused_status, refused) = delete_resource(&emulator, *fixture, None).await;
        assert_eq!(
            refused_status, 400,
            "case {case:?}: expected the first delete refused, got {refused_status} \
             with {refused:?}"
        );

        emulator
            .ok("DeleteDistribution", json!({"distributionName": "cdn-one"}))
            .await;

        let (status, body) = delete_resource(&emulator, *fixture, None).await;
        assert_eq!(
            status,
            kind.delete_success_status(),
            "case {case:?}: expected the delete served once no distribution pulls from \
             {name:?}, got {status} with {body:?}"
        );
        assert!(
            !resource_exists(&emulator, *fixture).await,
            "case {case:?}: the served delete must remove {name:?}"
        );
    }
}
