// SPDX-License-Identifier: CC0-1.0

//! `SetResourceAccessForBucket` — which Lightsail instances receive access to a
//! bucket — driven over the wire.
//!
//! Every case here goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.<Operation>` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handlers and the awsJson error envelope are all exercised rather than
//! bypassed. The harness is the one `instance_lifecycle.rs` already has, called
//! with a credential scope where a case needs a Region.
//!
//! The rule under test: the resources receiving access to a bucket are a SET on
//! that bucket's own record, keyed on the resource's name — `allow` puts the
//! instance the request names into it, `deny` takes it out, both resolved in the
//! Region the request is made in, and both idempotent. The set is what
//! `GetBuckets` reports as `resourcesReceivingAccess` to a caller that asks for
//! the connected resources, and what `DeleteBucket` counts when it refuses to
//! delete a bucket resources were granted access to.
//!
//! So the cases are TABLES: one row per Region a request can be made in
//! (including no credential scope at all), per bucket-name length at and inside
//! the modelled `BucketName` bounds, per number of instances granted access, and
//! per lifecycle state the grant is allowed from (`running` and `stopped`); and
//! one row per way a request can name a bucket, an instance or an access setting
//! the service does not serve.

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state as lightsail_state;

use crate::instance_lifecycle::{error_code, is_uuid_shaped, wait_for_state, Emulator};

/// A bundle every bucket in this suite is created on — the cheapest published
/// plan (`crate::state::BUCKET_BUNDLE_SPECS`); which plan a bucket is on decides
/// nothing about which resources receive access to it.
const BUCKET_BUNDLE: &str = "small_1_0";

/// The blueprint and bundle every instance in this suite is created on, from the
/// published catalogs (`crate::state::BLUEPRINT_SPECS` and
/// `crate::state::BUNDLE_SPECS`); neither decides anything about bucket access.
const BLUEPRINT: &str = "amazon_linux_2023";
const INSTANCE_BUNDLE: &str = "micro_3_0";

/// A name of exactly 54 characters — the upper bound the modelled `BucketName`
/// length (3..54) admits.
const LONGEST_BUCKET_NAME: &str = "abcdefghijabcdefghijabcdefghijabcdefghijabcdefghijabcd";

/// The `ResourceReceivingAccess$resourceType` of a granted instance: the modelled
/// `ResourceType` enum's `Instance` member.
const INSTANCE_RESOURCE_TYPE: &str = "Instance";

/// The two values the modelled `ResourceBucketAccess` enum admits
/// (`smithy_query(query="shape",shape="com.amazonaws.lightsail#ResourceBucketAccess")`).
const ALLOW: &str = "allow";
const DENY: &str = "deny";

/// Create the bucket `name` in `region`.
async fn create_bucket(emulator: &Emulator, region: Option<&str>, name: &str) {
    emulator
        .ok_at(
            None,
            region,
            "CreateBucket",
            json!({"bucketName": name, "bundleId": BUCKET_BUNDLE}),
        )
        .await;
}

/// Create the instance `name` in `zone` and leave it in `want` — `running`, the
/// state the clock owes a new instance, or `stopped` after a forced stop.
async fn create_instance(emulator: &Emulator, zone: &str, name: &str, want: &str) {
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [name],
                "availabilityZone": zone,
                "blueprintId": BLUEPRINT,
                "bundleId": INSTANCE_BUNDLE,
            }),
        )
        .await;
    wait_for_state(emulator, name, "running").await;
    if want == "stopped" {
        emulator
            .ok("StopInstance", json!({"instanceName": name, "force": true}))
            .await;
        wait_for_state(emulator, name, "stopped").await;
    }
}

/// Place `instance` in `state` in the store the handlers read, with no
/// transition owed, so a case about a state the API only passes THROUGH is not a
/// race against the clock.
///
/// The store is the only way to hold an instance in a transient state: every
/// path that reaches one (`CreateInstances`, `StartInstance`, `StopInstance`)
/// leaves the clock owing the settled state a moment later, which is what
/// `instance_start_addresses.rs` reaches the store for too.
fn hold_instance_state(emulator: &Emulator, instance: &str, state: lightsail_state::Lifecycle) {
    lightsail_state::with_state(emulator.engine(), |store| {
        let held = store
            .instance_mut(instance)
            .expect("the suite created this instance through CreateInstances");
        held.state = state;
        held.due = None;
    })
    .expect("the suite's engine is the shared in-memory one");
}

/// Set `access` for `instance` on `bucket` in `region`, answering the HTTP status
/// and parsed body.
async fn set_access(
    emulator: &Emulator,
    region: Option<&str>,
    bucket: &str,
    instance: &str,
    access: &str,
) -> (u16, Value) {
    emulator
        .call_in(
            region,
            "SetResourceAccessForBucket",
            json!({"bucketName": bucket, "resourceName": instance, "access": access}),
        )
        .await
}

/// Set `access` for `instance` on `bucket` in `region`, requiring success and
/// answering the `operations` the call reported.
async fn ok_set_access(
    emulator: &Emulator,
    region: Option<&str>,
    bucket: &str,
    instance: &str,
    access: &str,
) -> Value {
    emulator
        .ok_at(
            None,
            region,
            "SetResourceAccessForBucket",
            json!({"bucketName": bucket, "resourceName": instance, "access": access}),
        )
        .await["operations"]
        .clone()
}

/// The bucket `name` in `region`, as `GetBuckets` reports it — asking for the
/// connected resources when `connected`, so a case can pin both what the member
/// carries and that it is absent without the flag.
async fn read_bucket(
    emulator: &Emulator,
    region: Option<&str>,
    name: &str,
    connected: bool,
) -> Value {
    let reported = emulator
        .ok_at(
            None,
            region,
            "GetBuckets",
            json!({"bucketName": name, "includeConnectedResources": connected}),
        )
        .await;
    reported["buckets"][0].clone()
}

/// The names of the resources receiving access to `bucket` in `region`, in the
/// order the read reports them, asserting every entry is an `Instance`.
async fn granted(emulator: &Emulator, region: Option<&str>, bucket: &str) -> Vec<String> {
    let reported = read_bucket(emulator, region, bucket, true).await;
    let resources = reported["resourcesReceivingAccess"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case reading the resources receiving access to {bucket}: expected an array, \
                 got {reported:?}"
            )
        })
        .clone();
    resources
        .iter()
        .map(|resource| {
            assert_eq!(
                resource["resourceType"].as_str(),
                Some(INSTANCE_RESOURCE_TYPE),
                "case reading {bucket}: expected an Instance entry, got {resource:?}"
            );
            resource["name"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case reading {bucket}: every entry carries a name, got {resource:?}")
                })
                .to_string()
        })
        .collect()
}

/// Assert the one operation-log entry a `SetResourceAccessForBucket` call answers
/// with: it names the operation, the BUCKET whose record carries the change, and
/// is terminal because the access is set as the call returns.
fn assert_one_terminal_operation(case: &str, operations: &Value, bucket: &str) {
    let operations = operations.as_array().unwrap_or_else(|| {
        panic!("case {case}: expected an operations array, got {operations:?}")
    });
    assert_eq!(
        operations.len(),
        1,
        "case {case}: expected one operation for one access change, got {operations:?}"
    );
    let operation = &operations[0];
    assert_eq!(
        operation["operationType"].as_str(),
        Some("SetResourceAccessForBucket"),
        "case {case}: expected the SetResourceAccessForBucket operation type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceType"].as_str(),
        Some("Bucket"),
        "case {case}: expected the Bucket resource type, got {operation:?}"
    );
    assert_eq!(
        operation["resourceName"].as_str(),
        Some(bucket),
        "case {case}: expected the operation to name this bucket, got {operation:?}"
    );
    assert_eq!(
        operation["status"].as_str(),
        Some("Succeeded"),
        "case {case}: expected a terminal operation status, got {operation:?}"
    );
    assert_eq!(
        operation["isTerminal"].as_bool(),
        Some(true),
        "case {case}: expected a terminal operation, got {operation:?}"
    );
    let id = operation["id"].as_str().unwrap_or_default();
    assert!(
        is_uuid_shaped(id),
        "case {case}: expected a uuid-shaped operation id, got {id:?}"
    );
}

/// One row of the access table: where the bucket and the instances live, what the
/// bucket is called, which instances are granted access and which lifecycle state
/// they are in when the grant is made.
struct Row {
    case: &'static str,
    /// The Region the requests are made in, or `None` for requests that carry no
    /// credential scope at all (then the service's default Region).
    region: Option<&'static str>,
    /// An Availability Zone of that Region — an instance's Region is the Region
    /// of its zone.
    zone: &'static str,
    bucket: &'static str,
    instances: &'static [&'static str],
    /// The state the instances are in when access is set: `running` or `stopped`,
    /// the two the member's documentation admits.
    instance_state: &'static str,
}

/// The access cases. The rows differ along everything the rule generalises over:
/// the Region the calls are made in, the bucket name's length (both modelled
/// bounds and between them) and alphabet, how many instances are granted access,
/// and which of the two admitted lifecycle states they are in.
const ROWS: &[Row] = &[
    Row {
        case: "one running instance, no credential scope at all",
        region: None,
        zone: "us-east-1a",
        bucket: "access-default-region",
        instances: &["access-default-one"],
        instance_state: "running",
    },
    Row {
        case: "three running instances, in us-west-2",
        region: Some("us-west-2"),
        zone: "us-west-2b",
        bucket: "access-three-instances",
        instances: &["access-west-one", "access-west-two", "access-west-three"],
        instance_state: "running",
    },
    Row {
        case: "a stopped instance, in eu-west-1, shortest bucket name the model admits",
        region: Some("eu-west-1"),
        zone: "eu-west-1c",
        bucket: "abc",
        instances: &["access-eu-stopped"],
        instance_state: "stopped",
    },
    Row {
        case: "two instances, in ap-south-1, longest bucket name the model admits",
        region: Some("ap-south-1"),
        zone: "ap-south-1a",
        bucket: LONGEST_BUCKET_NAME,
        instances: &["access-south-one", "access-south-two"],
        instance_state: "running",
    },
    Row {
        case: "a stopped instance, in sa-east-1, a bucket name of digits and hyphens",
        region: Some("sa-east-1"),
        zone: "sa-east-1a",
        bucket: "0ne-2-3hree9",
        instances: &["access-sa-stopped"],
        instance_state: "stopped",
    },
];

/// RULE: `allow` adds the instance it names to the bucket's set of resources
/// receiving access and `deny` removes it, each exactly once however often it is
/// repeated, and every read of the bucket reports that stored set.
#[tokio::test]
async fn allow_and_deny_move_the_resources_receiving_access_of_one_bucket() {
    let emulator = Emulator::new();

    for row in ROWS {
        let case = row.case;
        create_bucket(&emulator, row.region, row.bucket).await;
        for instance in row.instances {
            create_instance(&emulator, row.zone, instance, row.instance_state).await;
        }

        // A bucket nothing was granted access to reports an empty list rather
        // than an absent one: the caller asked which resources receive access,
        // and the answer is "none".
        assert_eq!(
            granted(&emulator, row.region, row.bucket).await,
            Vec::<String>::new(),
            "case {case}: a bucket no resource was granted access to must report an empty list"
        );

        // Each `allow` adds exactly its own instance, and the entries stay in
        // the order access was given.
        let mut receiving: Vec<String> = Vec::new();
        for instance in row.instances {
            let operations = ok_set_access(&emulator, row.region, row.bucket, instance, ALLOW).await;
            assert_one_terminal_operation(case, &operations, row.bucket);
            receiving.push((*instance).to_string());
            assert_eq!(
                granted(&emulator, row.region, row.bucket).await,
                receiving,
                "case {case}: after allowing {instance} the bucket must report the grants so far"
            );
        }

        // `allow` for an instance that already has access changes nothing: the
        // set is keyed on the resource, so there is no second entry for it.
        let repeated = ok_set_access(&emulator, row.region, row.bucket, row.instances[0], ALLOW).await;
        assert_one_terminal_operation(case, &repeated, row.bucket);
        assert_eq!(
            granted(&emulator, row.region, row.bucket).await,
            receiving,
            "case {case}: allowing {} twice must leave one entry for it",
            row.instances[0]
        );

        // The member is reported only to a caller that asked for the connected
        // resources, which is what `GetBuckets$includeConnectedResources`
        // requests; a read without it reports the set not at all rather than
        // reporting it empty.
        let without = read_bucket(&emulator, row.region, row.bucket, false).await;
        assert!(
            without.get("resourcesReceivingAccess").is_none(),
            "case {case}: a read that did not ask for the connected resources must not report \
             them, got {without:?}"
        );

        // Reading changes nothing a later caller sees.
        assert_eq!(
            granted(&emulator, row.region, row.bucket).await,
            receiving,
            "case {case}: a second read must report the very same set"
        );

        // Each `deny` removes exactly the instance it names, leaving the rest;
        // the same `deny` again removes nothing and is still served.
        for instance in row.instances {
            let operations = ok_set_access(&emulator, row.region, row.bucket, instance, DENY).await;
            assert_one_terminal_operation(case, &operations, row.bucket);
            receiving.retain(|held| held != instance);
            assert_eq!(
                granted(&emulator, row.region, row.bucket).await,
                receiving,
                "case {case}: after denying {instance} the bucket must report the rest"
            );

            let again = ok_set_access(&emulator, row.region, row.bucket, instance, DENY).await;
            assert_one_terminal_operation(case, &again, row.bucket);
            assert_eq!(
                granted(&emulator, row.region, row.bucket).await,
                receiving,
                "case {case}: denying {instance} twice must change nothing"
            );
        }
        assert_eq!(
            granted(&emulator, row.region, row.bucket).await,
            Vec::<String>::new(),
            "case {case}: with every grant denied the bucket must report an empty list"
        );

        // The grants of one bucket are its own: granting access again after the
        // denials reaches the same one record.
        ok_set_access(&emulator, row.region, row.bucket, row.instances[0], ALLOW).await;
        assert_eq!(
            granted(&emulator, row.region, row.bucket).await,
            vec![row.instances[0].to_string()],
            "case {case}: the re-granted instance must be the one entry the bucket holds"
        );
    }
}

/// RULE: a bucket and an instance are resources of ONE Region, so a grant is made
/// between two resources of the Region the request is made in — a name that
/// addresses neither there is the service's `DoesNotExist` refusal, and a refused
/// request changes nothing.
#[tokio::test]
async fn a_grant_is_between_two_resources_of_one_region() {
    let emulator = Emulator::new();
    // One bucket and one instance in us-east-2, and one instance in eu-central-1
    // that the us-east-2 requests must not see.
    create_bucket(&emulator, Some("us-east-2"), "regional-access-bucket").await;
    create_instance(&emulator, "us-east-2a", "regional-access-instance", "running").await;
    create_instance(&emulator, "eu-central-1a", "elsewhere-access-instance", "running").await;

    // (case, the Region the call is made in, the bucket, the instance, the
    // resource type the refusal must name) — every way a request can name a
    // resource the Region it is made in does not hold.
    let absent: &[(&str, Option<&str>, &str, &str, &str)] = &[
        (
            "a bucket that was never created",
            Some("us-east-2"),
            "no-such-bucket",
            "regional-access-instance",
            "Bucket",
        ),
        (
            "an instance that was never created",
            Some("us-east-2"),
            "regional-access-bucket",
            "no-such-instance",
            "Instance",
        ),
        (
            "an instance of another Region",
            Some("us-east-2"),
            "regional-access-bucket",
            "elsewhere-access-instance",
            "Instance",
        ),
        (
            "the bucket of another Region",
            Some("eu-central-1"),
            "regional-access-bucket",
            "elsewhere-access-instance",
            "Bucket",
        ),
        (
            "both resources read from a Region that holds neither",
            Some("ap-northeast-1"),
            "regional-access-bucket",
            "regional-access-instance",
            "Bucket",
        ),
        (
            "no credential scope at all, so the default Region, which holds neither",
            None,
            "regional-access-bucket",
            "regional-access-instance",
            "Bucket",
        ),
    ];
    for (case, region, bucket, instance, resource_type) in absent {
        let (status, missing) = set_access(&emulator, *region, bucket, instance, ALLOW).await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {missing:?}"
        );
        assert_eq!(
            error_code(&missing),
            "NotFoundException",
            "case {case}: expected NotFoundException, got {missing:?}"
        );
        assert_eq!(
            missing["code"].as_str(),
            Some("DoesNotExist"),
            "case {case}: expected the body code DoesNotExist, got {missing:?}"
        );
        assert_eq!(
            missing["message"].as_str(),
            Some(
                format!(
                    "The {resource_type} does not exist: {}",
                    if *resource_type == "Bucket" {
                        bucket
                    } else {
                        instance
                    }
                )
                .as_str()
            ),
            "case {case}: expected the message to name the missing resource, got {missing:?}"
        );
        assert_eq!(
            granted(&emulator, Some("us-east-2"), "regional-access-bucket").await,
            Vec::<String>::new(),
            "case {case}: a refused request must grant nothing"
        );
    }

    // The same two resources, in the Region that holds them, ARE granted.
    ok_set_access(
        &emulator,
        Some("us-east-2"),
        "regional-access-bucket",
        "regional-access-instance",
        ALLOW,
    )
    .await;
    assert_eq!(
        granted(&emulator, Some("us-east-2"), "regional-access-bucket").await,
        vec!["regional-access-instance".to_string()],
        "case the Region that holds both: the grant must be stored"
    );
}

/// RULE: access is set only for an instance in a running or stopped state, and
/// only for an access setting the modelled `ResourceBucketAccess` enum admits;
/// each refusal is a 400 that changes nothing.
#[tokio::test]
async fn an_unservable_request_is_refused_and_grants_nothing() {
    let emulator = Emulator::new();
    create_bucket(&emulator, None, "refusing-access-bucket").await;
    create_instance(&emulator, "us-east-1a", "refusing-access-instance", "running").await;

    // An access setting outside the modelled enum is refused by the centralized
    // constraint walk before the handler runs, which is why these rows expect
    // that refusal's own wire code rather than the service's.
    let outside_the_enum: &[(&str, &str)] = &[
        ("an access setting of the wrong case", "Allow"),
        ("an access setting the enum has no member for", "revoke"),
        ("an empty access setting", ""),
    ];
    for (case, access) in outside_the_enum {
        let (status, refused) = set_access(
            &emulator,
            None,
            "refusing-access-bucket",
            "refusing-access-instance",
            access,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400 for access {access:?}, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "ValidationException",
            "case {case}: expected ValidationException for access {access:?}, got {refused:?}"
        );
        assert_eq!(
            granted(&emulator, None, "refusing-access-bucket").await,
            Vec::<String>::new(),
            "case {case}: a refused request must grant nothing"
        );
    }

    // Every state the modelled `InstanceState` has that is neither running nor
    // stopped. Each is placed in the store the handlers read, with no transition
    // owed, so the case meets that state however fast or slow the host is —
    // racing the create's own `pending` window would make the refusal it
    // exercises depend on the clock.
    for (case, placed) in [
        ("an instance still coming up", lightsail_state::PENDING),
        ("an instance on its way down", lightsail_state::STOPPING),
    ] {
        hold_instance_state(&emulator, "refusing-access-instance", placed);
        let (status, refused) = set_access(
            &emulator,
            None,
            "refusing-access-bucket",
            "refusing-access-instance",
            ALLOW,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected a 400, got {status} {refused:?}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidInputException",
            "case {case}: expected InvalidInputException, got {refused:?}"
        );
        // The refusal names the state the instance is actually in, so the caller
        // learns what to wait for.
        assert!(
            refused["message"]
                .as_str()
                .unwrap_or_default()
                .contains(placed.name),
            "case {case}: expected the message to name the state {:?}, got {refused:?}",
            placed.name
        );
        assert_eq!(
            granted(&emulator, None, "refusing-access-bucket").await,
            Vec::<String>::new(),
            "case {case}: a refused request must grant nothing"
        );
    }

    // The very same request against the very same names, once that instance is
    // in one of the two states the member admits: what was refused was the
    // state, not the names. Both admitted states are reached through the API —
    // `StartInstance` for `running`, a forced `StopInstance` for `stopped`.
    for (case, operation, body, want) in [
        (
            "the instance started again",
            "StartInstance",
            json!({"instanceName": "refusing-access-instance"}),
            "running",
        ),
        (
            "the instance stopped",
            "StopInstance",
            json!({"instanceName": "refusing-access-instance", "force": true}),
            "stopped",
        ),
    ] {
        emulator.ok(operation, body).await;
        wait_for_state(&emulator, "refusing-access-instance", want).await;
        ok_set_access(
            &emulator,
            None,
            "refusing-access-bucket",
            "refusing-access-instance",
            ALLOW,
        )
        .await;
        assert_eq!(
            granted(&emulator, None, "refusing-access-bucket").await,
            vec!["refusing-access-instance".to_string()],
            "case {case}: the grant must be stored for an instance that is {want}"
        );
    }
}

/// RULE: the grant is REAL state of the bucket, so another operation reading the
/// same record sees it — `DeleteBucket` requires `forceDelete` for a bucket
/// "that has instances that were granted access to it using the
/// SetResourceAccessForBucket action", and stops requiring it once the access is
/// denied again.
#[tokio::test]
async fn a_granted_instance_blocks_an_unforced_bucket_delete() {
    let emulator = Emulator::new();
    create_bucket(&emulator, None, "guarded-access-bucket").await;
    create_instance(&emulator, "us-east-1a", "guarded-access-instance", "running").await;
    ok_set_access(
        &emulator,
        None,
        "guarded-access-bucket",
        "guarded-access-instance",
        ALLOW,
    )
    .await;

    let (status, refused) = emulator
        .call_in(None, "DeleteBucket", json!({"bucketName": "guarded-access-bucket"}))
        .await;
    assert_eq!(
        status, 400,
        "case a granted instance: expected an unforced delete to be refused, got {status} \
         {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidInputException",
        "case a granted instance: expected InvalidInputException, got {refused:?}"
    );
    assert_eq!(
        granted(&emulator, None, "guarded-access-bucket").await,
        vec!["guarded-access-instance".to_string()],
        "case a granted instance: the refused delete left the grant in place"
    );

    // With the access denied the bucket holds nothing a force delete is required
    // for, so the same delete is served.
    ok_set_access(
        &emulator,
        None,
        "guarded-access-bucket",
        "guarded-access-instance",
        DENY,
    )
    .await;
    emulator
        .ok("DeleteBucket", json!({"bucketName": "guarded-access-bucket"}))
        .await;
    let (status, gone) = emulator
        .call_in(None, "GetBuckets", json!({"bucketName": "guarded-access-bucket"}))
        .await;
    assert_eq!(
        status, 400,
        "case the denied grant: expected the deleted bucket to be gone, got {status} {gone:?}"
    );
    assert_eq!(
        error_code(&gone),
        "NotFoundException",
        "case the denied grant: expected NotFoundException for the deleted bucket, got {gone:?}"
    );
}
