// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Security Lake data-lake + subscriber slice.
//!
//! Every case drives a real `restJson1` request through the router into this service, so
//! the routes, the wire shapes, the status codes and the persisted state are all under
//! test — nothing reaches into the state struct directly.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_securitylake_securitylake::state::{
    AccountRole, ComAmazonawsSecuritylakeSecuritylakeState as Store,
};
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account id (`engine::EMULATOR_ACCOUNT_ID`): every ARN this
/// service mints carries it.
const ACCOUNT_ID: &str = "000000000000";

/// The SigV4 access key id every request this harness sends presents, which is the
/// principal this service's authorization records are keyed by
/// (`state::caller_principal`). `pub(crate)` so a suite beside this one can make this
/// harness's caller a refused one.
pub(crate) const CALLER_PRINCIPAL: &str = "AKIDTESTTESTTEST";

/// A `RoleArn`-shaped value, matching `CreateDataLakeRequest$metaStoreManagerRoleArn`'s
/// pattern `^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`.
const META_STORE_ROLE: &str = "arn:aws:iam::000000000000:role/AmazonSecurityLakeMetaStoreManager";

/// `AwsLogSourceName`'s wire values, from
/// `service-com-amazonaws-securitylake-securitylake/src/ops/types/shared_types.rs`
/// (`AWS_LOG_SOURCE_NAME_VALUES`, generated from the model's enum).
const AWS_LOG_SOURCE_NAMES: &[&str] = &[
    "CLOUD_TRAIL_MGMT",
    "EKS_AUDIT",
    "LAMBDA_EXECUTION",
    "ROUTE53",
    "S3_DATA",
    "SH_FINDINGS",
    "VPC_FLOW",
    "WAF",
];

/// One response, as a test reads it.
///
/// `pub(crate)` so the suites beside this one in `tests/all/` drive the service through
/// the SAME harness instead of each growing its own copy.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) error_type: Option<String>,
    pub(crate) body: Value,
}

/// The emulator under test: this one service mounted on the production router.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The shared engine, so a case can set the account-topology records the control
    /// plane decides on — the one `Arc` the service holds too, never a second store.
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Request signatures are verified by the emulator, not by the service; these
        // tests exercise the service, so the authenticator is off and the Authorization
        // header is present only to carry the route key and the Region.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_securitylake_securitylake::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        let emulator = Self { router, engine };
        // Security Lake's control plane only serves an account that administers a data
        // lake — the organization's delegated administrator or a standalone account — and
        // an account holds no such role until one is given to it. The cases on this
        // harness are about what the data-lake and subscriber operations DO, so the
        // account topology is seeded here, through the store that holds it: no modelled
        // operation writes it (`RegisterDataLakeDelegatedAdministrator` is the
        // organization management account's call). The gate itself is under test in
        // `account_entitlement_gate`, which sets the role back through the accessor
        // below.
        emulator.set_account_role(AccountRole::Standalone);
        emulator
    }

    /// Give the caller account `role`, answering the role it held before. The account
    /// topology is a record of this service's store, like its authorization records.
    pub(crate) fn set_account_role(&self, role: AccountRole) -> AccountRole {
        self.store().lock().set_account_role(role)
    }

    /// This service's store in the shared engine — the one the service itself reads,
    /// never a second one. `pub(crate)` so a suite beside this one can seed a record no
    /// modelled operation writes (the account topology above, or the failure a Region's
    /// data lake has recorded).
    pub(crate) fn store(&self) -> ::engine::ServiceState<Store> {
        self.engine.state::<Store>()
    }

    pub(crate) async fn send(
        &self,
        method: &str,
        path: &str,
        region: &str,
        body: Option<Value>,
    ) -> Reply {
        // The SigV4 credential scope is what names both the service and the Region.
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential={CALLER_PRINCIPAL}/20260101/{region}/securitylake/aws4_request, \
             SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
        );
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", authorization)
            .header("host", "securitylake.us-east-1.amazonaws.com")
            .header("content-type", "application/json")
            .body(payload)
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_type = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        Reply {
            status,
            error_type,
            body,
        }
    }

    pub(crate) async fn create_data_lake(&self, region: &str, configuration: Value) -> Reply {
        self.send(
            "POST",
            "/v1/datalake",
            region,
            Some(json!({
                "configurations": [configuration],
                "metaStoreManagerRoleArn": META_STORE_ROLE,
            })),
        )
        .await
    }

    pub(crate) async fn list_data_lakes(&self, region: &str, query: &str) -> Reply {
        self.send("GET", &format!("/v1/datalakes{query}"), region, None)
            .await
    }

    pub(crate) async fn create_subscriber(&self, region: &str, body: Value) -> Reply {
        self.send("POST", "/v1/subscribers", region, Some(body)).await
    }

    pub(crate) async fn get_subscriber(&self, region: &str, subscriber_id: &str) -> Reply {
        self.send(
            "GET",
            &format!("/v1/subscribers/{subscriber_id}"),
            region,
            None,
        )
        .await
    }

    pub(crate) async fn list_subscribers(&self, region: &str, query: &str) -> Reply {
        self.send("GET", &format!("/v1/subscribers{query}"), region, None)
            .await
    }

    pub(crate) async fn delete_subscriber(&self, region: &str, subscriber_id: &str) -> Reply {
        self.send(
            "DELETE",
            &format!("/v1/subscribers/{subscriber_id}"),
            region,
            None,
        )
        .await
    }

    pub(crate) async fn delete_data_lake(&self, region: &str, regions: &[&str]) -> Reply {
        self.send(
            "POST",
            "/v1/datalake/delete",
            region,
            Some(json!({ "regions": regions })),
        )
        .await
    }
}

/// `^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$` — the pattern the
/// model's `UUID` shape (and so `SubscriberResource$subscriberId`) carries.
fn is_model_uuid(candidate: &str) -> bool {
    let groups: Vec<&str> = candidate.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(group, len)| group.len() == len)
        && groups
            .iter()
            .all(|group| group.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')))
}

pub(crate) fn array<'a>(reply: &'a Reply, member: &str, case: &str) -> &'a Vec<Value> {
    reply.body[member]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected `{member}` to be a list, got {:?}", reply.body))
}

pub(crate) fn text<'a>(value: &'a Value, member: &str, case: &str) -> &'a str {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected `{member}` to be a string, got {value:?}"))
}

/// The regions of the data lakes a ListDataLakes reply reports, in order.
fn reported_regions(reply: &Reply, case: &str) -> Vec<String> {
    array(reply, "dataLakes", case)
        .iter()
        .map(|lake| text(lake, "region", case).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// CreateDataLake / ListDataLakes
// ---------------------------------------------------------------------------

/// RULE: CreateDataLake persists one data lake per entry in `configurations`, keyed by its
/// Region, and reports each one from the store with a Region-derived data-lake ARN and S3
/// bucket ARN plus the sub-configurations as stored.
///
/// The rows differ in the Region AND in which optional sub-configurations are supplied.
#[tokio::test]
async fn create_data_lake_stores_one_lake_per_region_and_reports_it_from_the_store() {
    let cases: Vec<(&str, Value)> = vec![
        ("us-east-1", json!({ "region": "us-east-1" })),
        (
            "eu-west-2",
            json!({
                "region": "eu-west-2",
                "encryptionConfiguration": { "kmsKeyId": "alias/aws/securitylake" },
            }),
        ),
        (
            "ap-southeast-1",
            json!({
                "region": "ap-southeast-1",
                "lifecycleConfiguration": {
                    "expiration": { "days": 365 },
                    "transitions": [{ "storageClass": "ONEZONE_IA", "days": 30 }],
                },
            }),
        ),
        (
            "sa-east-1",
            json!({
                "region": "sa-east-1",
                "replicationConfiguration": { "regions": ["us-east-1"], "roleArn": META_STORE_ROLE },
            }),
        ),
    ];

    let emulator = Emulator::new();
    for (region, configuration) in &cases {
        let reply = emulator.create_data_lake(region, configuration.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );

        let lakes = array(&reply, "dataLakes", region);
        assert_eq!(
            lakes.len(),
            1,
            "case {region:?}: expected one data lake, got {lakes:?}"
        );
        let lake = &lakes[0];

        assert_eq!(
            text(lake, "dataLakeArn", region),
            format!("arn:aws:securitylake:{region}:{ACCOUNT_ID}:data-lake/default"),
            "case {region:?}: wrong dataLakeArn, got {lake:?}"
        );
        assert_eq!(
            text(lake, "region", region),
            *region,
            "case {region:?}: wrong region, got {lake:?}"
        );
        // The clock-owed INITIALIZED -> COMPLETED transition is asserted strictly in its
        // own suite below; here only the fact that a stored status is reported matters, so
        // this row cannot flip on a loaded host.
        let status = text(lake, "createStatus", region);
        assert!(
            status == "INITIALIZED" || status == "COMPLETED",
            "case {region:?}: expected a DataLakeStatus of INITIALIZED or COMPLETED, got {status:?}"
        );

        let prefix = format!("arn:aws:s3:::aws-security-data-lake-{region}-");
        let bucket = text(lake, "s3BucketArn", region);
        let suffix = bucket.strip_prefix(&prefix).unwrap_or_else(|| {
            panic!("case {region:?}: expected s3BucketArn to start with {prefix:?}, got {bucket:?}")
        });
        assert!(
            suffix.len() == 12 && suffix.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "case {region:?}: expected a 12-character hex bucket suffix, got {suffix:?}"
        );

        for member in [
            "encryptionConfiguration",
            "lifecycleConfiguration",
            "replicationConfiguration",
        ] {
            assert_eq!(
                lake[member], configuration[member],
                "case {region:?}: `{member}` must come back as stored, got {:?}",
                lake[member]
            );
        }
    }

    // Every Region created above is now reported, each keeping its own identity.
    let reply = emulator.list_data_lakes("us-east-1", "").await;
    assert_eq!(reply.status, 200, "ListDataLakes: expected 200, got {}", reply.status);
    let expected: Vec<String> = cases.iter().map(|(region, _)| (*region).to_string()).collect();
    assert_eq!(
        reported_regions(&reply, "all regions"),
        expected,
        "case \"all regions\": ListDataLakes must report every stored Region, got {:?}",
        reply.body
    );
    for lake in array(&reply, "dataLakes", "all regions") {
        let region = text(lake, "region", "all regions");
        assert_eq!(
            text(lake, "dataLakeArn", "all regions"),
            format!("arn:aws:securitylake:{region}:{ACCOUNT_ID}:data-lake/default"),
            "case {region:?}: ListDataLakes must report the stored ARN, got {lake:?}"
        );
    }
}

/// RULE: a CreateDataLake for a Region that already has a data lake UPDATES that Region
/// with the sub-configurations the request carries and leaves the others as stored — the
/// model documents the operation as an upsert ("If you have already enabled Security Lake
/// in a Region when you call this command, the command will update the Region if you
/// provide new configuration parameters").
///
/// The rows differ in which sub-configuration the second call supplies.
#[tokio::test]
async fn re_creating_a_region_updates_only_what_the_request_carries() {
    let first = json!({
        "region": "us-east-1",
        "encryptionConfiguration": { "kmsKeyId": "alias/original" },
        "lifecycleConfiguration": { "expiration": { "days": 90 } },
    });

    let cases: Vec<(&str, Value, Value, Value)> = vec![
        (
            "nothing new",
            json!({ "region": "us-east-1" }),
            json!({ "kmsKeyId": "alias/original" }),
            json!({ "expiration": { "days": 90 } }),
        ),
        (
            "new encryption only",
            json!({
                "region": "us-east-1",
                "encryptionConfiguration": { "kmsKeyId": "alias/rotated" },
            }),
            json!({ "kmsKeyId": "alias/rotated" }),
            json!({ "expiration": { "days": 90 } }),
        ),
        (
            "new lifecycle only",
            json!({
                "region": "us-east-1",
                "lifecycleConfiguration": { "expiration": { "days": 730 } },
            }),
            json!({ "kmsKeyId": "alias/original" }),
            json!({ "expiration": { "days": 730 } }),
        ),
        (
            "new encryption and lifecycle",
            json!({
                "region": "us-east-1",
                "encryptionConfiguration": { "kmsKeyId": "alias/rotated" },
                "lifecycleConfiguration": { "expiration": { "days": 730 } },
            }),
            json!({ "kmsKeyId": "alias/rotated" }),
            json!({ "expiration": { "days": 730 } }),
        ),
    ];

    for (case, second, expected_encryption, expected_lifecycle) in &cases {
        let emulator = Emulator::new();
        let created = emulator.create_data_lake("us-east-1", first.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the first CreateDataLake must succeed, got {}",
            created.status
        );

        let reply = emulator.create_data_lake("us-east-1", second.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: re-creating an enabled Region updates it, got {} with {:?}",
            reply.status, reply.body
        );

        // Both the response and a later read must show the merged configuration.
        let listed = emulator.list_data_lakes("us-east-1", "").await;
        assert_eq!(
            reported_regions(&listed, case),
            vec!["us-east-1".to_string()],
            "case {case:?}: the Region must not be duplicated, got {:?}",
            listed.body
        );
        for (source, lake) in [
            ("CreateDataLake", &array(&reply, "dataLakes", case)[0]),
            ("ListDataLakes", &array(&listed, "dataLakes", case)[0]),
        ] {
            assert_eq!(
                lake["encryptionConfiguration"], *expected_encryption,
                "case {case:?} ({source}): wrong encryptionConfiguration, got {:?}",
                lake["encryptionConfiguration"]
            );
            assert_eq!(
                lake["lifecycleConfiguration"], *expected_lifecycle,
                "case {case:?} ({source}): wrong lifecycleConfiguration, got {:?}",
                lake["lifecycleConfiguration"]
            );
        }
    }
}

/// RULE: ListDataLakes reports the stored data lakes narrowed to the `regions` the caller
/// asked for, and an empty list — not an error — when none match.
#[tokio::test]
async fn list_data_lakes_filters_by_the_requested_regions() {
    let emulator = Emulator::new();
    for region in ["us-east-1", "eu-west-2", "sa-east-1"] {
        let reply = emulator
            .create_data_lake(region, json!({ "region": region }))
            .await;
        assert_eq!(reply.status, 200, "setting up {region:?}: got {}", reply.status);
    }

    let cases: Vec<(&str, &str, Vec<&str>)> = vec![
        ("no filter", "", vec!["us-east-1", "eu-west-2", "sa-east-1"]),
        ("one region", "?regions=eu-west-2", vec!["eu-west-2"]),
        (
            "two regions",
            "?regions=us-east-1&regions=sa-east-1",
            vec!["us-east-1", "sa-east-1"],
        ),
        ("a region with no data lake", "?regions=ca-central-1", vec![]),
    ];

    for (case, query, expected) in &cases {
        let reply = emulator.list_data_lakes("us-east-1", query).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let expected: Vec<String> = expected.iter().map(|r| (*r).to_string()).collect();
        assert_eq!(
            reported_regions(&reply, case),
            expected,
            "case {case:?}: wrong regions reported, got {:?}",
            reply.body
        );
    }

    // And a store with nothing in it is an empty list, not an error.
    let empty = Emulator::new();
    let reply = empty.list_data_lakes("us-east-1", "").await;
    assert_eq!(
        reply.status, 200,
        "case \"nothing created\": expected 200, got {}",
        reply.status
    );
    assert!(
        array(&reply, "dataLakes", "nothing created").is_empty(),
        "case \"nothing created\": expected an empty dataLakes list, got {:?}",
        reply.body
    );
}

/// RULE: the INITIALIZED -> COMPLETED transition a create owes is due on the CLOCK, so a
/// caller reading inside the span still sees INITIALIZED and a caller reading after it sees
/// COMPLETED, which then stays put.
///
/// The transient assertions are keyed off the SAME span the service uses and the elapsed
/// time measured here, so a host slow enough to cross the span cannot make this flake.
#[tokio::test]
async fn a_new_data_lake_settles_from_initialized_to_completed_on_the_clock() {
    use service_com_amazonaws_securitylake_securitylake::state::DATA_LAKE_COMPLETION_DELAY;

    let emulator = Emulator::new();
    let started = std::time::Instant::now();
    let created = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    let create_status =
        text(&array(&created, "dataLakes", "immediately")[0], "createStatus", "immediately")
            .to_string();

    let straight_after = emulator.list_data_lakes("us-east-1", "").await;
    let list_status = text(
        &array(&straight_after, "dataLakes", "straight after")[0],
        "createStatus",
        "straight after",
    )
    .to_string();
    let elapsed = started.elapsed();

    // Inside the span the transient status MUST still be observable — that is what makes
    // the span non-zero. Past it, the clock already owed COMPLETED, so only the stable
    // assertions below apply.
    if elapsed < DATA_LAKE_COMPLETION_DELAY {
        for (case, status) in [
            ("immediately", &create_status),
            ("straight after", &list_status),
        ] {
            assert_eq!(
                status, "INITIALIZED",
                "case {case:?}: within the {DATA_LAKE_COMPLETION_DELAY:?} span (at {elapsed:?}) \
                 the transient status must be reported, got {status:?}"
            );
        }
    }

    // The span is stored as a due instant, never slept on by the handler; the test is the
    // only thing that waits. `std::thread::sleep` rather than `tokio::time::sleep` because
    // this workspace's `tokio` dependency does not enable the `time` feature.
    std::thread::sleep(DATA_LAKE_COMPLETION_DELAY + Duration::from_millis(400));

    for case in ["after the span", "and again"] {
        let reply = emulator.list_data_lakes("us-east-1", "").await;
        assert_eq!(
            text(&array(&reply, "dataLakes", case)[0], "createStatus", case),
            "COMPLETED",
            "case {case:?}: the clock-owed transition must have been applied, got {:?}",
            reply.body
        );
    }
}

/// RULE: the response carries one entry per REGION, not one per request entry, and a
/// `configurations` list longer than the number of data lakes this emulator can hold is
/// refused `BadRequestException` (400) — a fixed limit, so a retry could never succeed.
#[tokio::test]
async fn create_data_lake_answers_per_region_and_bounds_the_request_list() {
    use service_com_amazonaws_securitylake_securitylake::state::MAX_DATA_LAKES;

    // Rows differ in how the request repeats a Region: twice with disjoint
    // sub-configurations, twice with the same one, and three distinct Regions.
    let cases: Vec<(&str, Value, usize)> = vec![
        (
            "one region twice, disjoint sub-configurations",
            json!([
                { "region": "us-east-1", "encryptionConfiguration": { "kmsKeyId": "alias/a" } },
                { "region": "us-east-1", "lifecycleConfiguration": { "expiration": { "days": 30 } } },
            ]),
            1,
        ),
        (
            "one region three times",
            json!([
                { "region": "eu-west-2" },
                { "region": "eu-west-2" },
                { "region": "eu-west-2" },
            ]),
            1,
        ),
        (
            "three distinct regions",
            json!([
                { "region": "us-east-1" },
                { "region": "eu-west-2" },
                { "region": "sa-east-1" },
            ]),
            3,
        ),
    ];

    for (case, configurations, expected_entries) in &cases {
        let emulator = Emulator::new();
        let reply = emulator
            .send(
                "POST",
                "/v1/datalake",
                "us-east-1",
                Some(json!({
                    "configurations": configurations,
                    "metaStoreManagerRoleArn": META_STORE_ROLE,
                })),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            array(&reply, "dataLakes", case).len(),
            *expected_entries,
            "case {case:?}: expected one entry per Region, got {:?}",
            reply.body
        );
        // And the store agrees: no Region is held twice.
        let listed = emulator.list_data_lakes("us-east-1", "").await;
        assert_eq!(
            reported_regions(&listed, case).len(),
            *expected_entries,
            "case {case:?}: the store must hold one data lake per Region, got {:?}",
            listed.body
        );
    }

    // Applying the entries in order leaves every sub-configuration the request supplied,
    // exactly as two sequential calls would.
    let emulator = Emulator::new();
    let merged = emulator
        .send(
            "POST",
            "/v1/datalake",
            "us-east-1",
            Some(json!({
                "configurations": [
                    { "region": "us-east-1", "encryptionConfiguration": { "kmsKeyId": "alias/a" } },
                    { "region": "us-east-1", "lifecycleConfiguration": { "expiration": { "days": 30 } } },
                ],
                "metaStoreManagerRoleArn": META_STORE_ROLE,
            })),
        )
        .await;
    let lake = &array(&merged, "dataLakes", "merged")[0];
    assert_eq!(
        lake["encryptionConfiguration"],
        json!({ "kmsKeyId": "alias/a" }),
        "case \"merged\": the first entry's sub-configuration must survive, got {lake:?}"
    );
    assert_eq!(
        lake["lifecycleConfiguration"],
        json!({ "expiration": { "days": 30 } }),
        "case \"merged\": the second entry's sub-configuration must be applied, got {lake:?}"
    );

    // A list longer than the emulator can ever usefully hold is refused outright.
    let too_many: Vec<Value> = (1..=MAX_DATA_LAKES + 1)
        .map(|index| json!({ "region": format!("us-east-{index}") }))
        .collect();
    let reply = emulator
        .send(
            "POST",
            "/v1/datalake",
            "us-east-1",
            Some(json!({
                "configurations": too_many,
                "metaStoreManagerRoleArn": META_STORE_ROLE,
            })),
        )
        .await;
    assert_eq!(
        reply.status, 400,
        "case \"an over-long configurations list\": expected 400, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("BadRequestException"),
        "case \"an over-long configurations list\": wrong error code, got {:?}",
        reply.error_type
    );
}

// ---------------------------------------------------------------------------
// Subscribers
// ---------------------------------------------------------------------------

/// RULE: CreateSubscriber mints a subscriber for the data lake of the Region the request
/// was signed for, and GetSubscriber answers with exactly the record that was stored; the
/// access-dependent members (`roleArn`/`s3BucketArn` for `S3`, `resourceShareArn`/
/// `resourceShareName` for `LAKEFORMATION`) follow the `accessTypes` that were asked for.
///
/// The rows differ in the access types, in the kind and count of `sources`, and in the
/// principal form `AwsIdentity$principal` accepts.
#[tokio::test]
async fn a_created_subscriber_reads_back_identically_across_access_types_and_sources() {
    #[allow(clippy::type_complexity)]
    let cases: Vec<(&str, Option<Vec<&str>>, Value, &str)> = vec![
        (
            "s3 only, one aws source",
            Some(vec!["S3"]),
            json!([{ "awsLogSource": { "sourceName": "ROUTE53", "sourceVersion": "2.0" } }]),
            "123456789012",
        ),
        (
            "lake formation only, one aws source",
            Some(vec!["LAKEFORMATION"]),
            json!([{ "awsLogSource": { "sourceName": "VPC_FLOW", "sourceVersion": "latest" } }]),
            "000000000000",
        ),
        (
            "both access types, two sources of both kinds",
            Some(vec!["S3", "LAKEFORMATION"]),
            json!([
                { "awsLogSource": { "sourceName": "EKS_AUDIT", "sourceVersion": "2.0" } },
                { "customLogSource": {
                    "sourceName": "my_custom_source",
                    "sourceVersion": "1.0",
                    "provider": { "roleArn": META_STORE_ROLE, "location": "s3://bucket/prefix/" },
                } },
            ]),
            "securitylake.amazonaws.com",
        ),
        (
            "no access types, one custom source",
            None,
            json!([{ "customLogSource": { "sourceName": "another_source", "sourceVersion": "9.9" } }]),
            "123456789012",
        ),
    ];

    for (case, access_types, sources, principal) in &cases {
        let emulator = Emulator::new();
        let created_lake = emulator
            .create_data_lake("eu-west-2", json!({ "region": "eu-west-2" }))
            .await;
        assert_eq!(
            created_lake.status, 200,
            "case {case:?}: setting up the data lake failed with {}",
            created_lake.status
        );
        let lake_bucket = text(
            &array(&created_lake, "dataLakes", case)[0],
            "s3BucketArn",
            case,
        )
        .to_string();

        let mut request = json!({
            "subscriberIdentity": { "principal": principal, "externalId": "ext-1" },
            "subscriberName": "sub-one",
            "subscriberDescription": "a_subscriber",
            "sources": sources,
        });
        if let Some(access_types) = access_types {
            request["accessTypes"] = json!(access_types);
        }

        let reply = emulator.create_subscriber("eu-west-2", request.clone()).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let subscriber = reply.body["subscriber"].clone();

        let subscriber_id = text(&subscriber, "subscriberId", case);
        assert!(
            is_model_uuid(subscriber_id),
            "case {case:?}: subscriberId must match the model's UUID pattern, got {subscriber_id:?}"
        );
        assert_eq!(
            text(&subscriber, "subscriberArn", case),
            format!("arn:aws:securitylake:eu-west-2:{ACCOUNT_ID}:subscriber/{subscriber_id}"),
            "case {case:?}: wrong subscriberArn, got {subscriber:?}"
        );
        assert_eq!(
            subscriber["subscriberIdentity"], request["subscriberIdentity"],
            "case {case:?}: subscriberIdentity must come back as stored, got {subscriber:?}"
        );
        assert_eq!(
            subscriber["subscriberName"], request["subscriberName"],
            "case {case:?}: subscriberName must come back as stored, got {subscriber:?}"
        );
        assert_eq!(
            subscriber["subscriberDescription"], request["subscriberDescription"],
            "case {case:?}: subscriberDescription must come back as stored, got {subscriber:?}"
        );
        assert_eq!(
            subscriber["sources"], *sources,
            "case {case:?}: sources must come back as stored, got {subscriber:?}"
        );
        assert_eq!(
            text(&subscriber, "subscriberStatus", case),
            "ACTIVE",
            "case {case:?}: wrong subscriberStatus, got {subscriber:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                !subscriber[member].is_null(),
                "case {case:?}: `{member}` must be set, got {subscriber:?}"
            );
        }

        let asked = access_types.clone().unwrap_or_default();
        assert_eq!(
            subscriber["accessTypes"],
            if asked.is_empty() {
                Value::Null
            } else {
                json!(asked)
            },
            "case {case:?}: wrong accessTypes, got {subscriber:?}"
        );

        if asked.contains(&"S3") {
            assert_eq!(
                text(&subscriber, "roleArn", case),
                format!("arn:aws:iam::{ACCOUNT_ID}:role/AmazonSecurityLake-{subscriber_id}"),
                "case {case:?}: an S3 subscriber gets its own role, got {subscriber:?}"
            );
            assert_eq!(
                text(&subscriber, "s3BucketArn", case),
                lake_bucket,
                "case {case:?}: an S3 subscriber reads the data lake's own bucket, got {subscriber:?}"
            );
        } else {
            for member in ["roleArn", "s3BucketArn"] {
                assert!(
                    subscriber[member].is_null(),
                    "case {case:?}: `{member}` is only for S3 access, got {subscriber:?}"
                );
            }
        }

        if asked.contains(&"LAKEFORMATION") {
            assert_eq!(
                text(&subscriber, "resourceShareArn", case),
                format!("arn:aws:ram:eu-west-2:{ACCOUNT_ID}:resource-share/{subscriber_id}"),
                "case {case:?}: a LAKEFORMATION subscriber gets a resource share, got {subscriber:?}"
            );
            assert_eq!(
                text(&subscriber, "resourceShareName", case),
                format!("LakeFormation-V1-{ACCOUNT_ID}-sub-one"),
                "case {case:?}: wrong resourceShareName, got {subscriber:?}"
            );
        } else {
            for member in ["resourceShareArn", "resourceShareName"] {
                assert!(
                    subscriber[member].is_null(),
                    "case {case:?}: `{member}` is only for LAKEFORMATION access, got {subscriber:?}"
                );
            }
        }

        // THE ROUND TRIP: the reader answers with exactly what the create stored.
        let fetched = emulator.get_subscriber("eu-west-2", subscriber_id).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: GetSubscriber expected 200, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.body["subscriber"], subscriber,
            "case {case:?}: GetSubscriber must return the stored record, got {:?}",
            fetched.body
        );
    }
}

/// RULE: every `AwsLogSourceName` the model declares is an acceptable `sources` entry and
/// is stored verbatim — including the ones no issue cited.
#[tokio::test]
async fn every_modelled_aws_log_source_name_round_trips() {
    let emulator = Emulator::new();
    let created_lake = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(
        created_lake.status, 200,
        "setting up the data lake failed with {}",
        created_lake.status
    );

    for source_name in AWS_LOG_SOURCE_NAMES {
        let sources = json!([{ "awsLogSource": { "sourceName": source_name, "sourceVersion": "latest" } }]);
        let reply = emulator
            .create_subscriber(
                "us-east-1",
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                    "subscriberName": format!("sub-{}", source_name.to_lowercase()),
                    "sources": sources,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {source_name:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        let subscriber = reply.body["subscriber"].clone();
        assert_eq!(
            subscriber["sources"], sources,
            "case {source_name:?}: sources must come back as stored, got {subscriber:?}"
        );

        let fetched = emulator
            .get_subscriber("us-east-1", text(&subscriber, "subscriberId", source_name))
            .await;
        assert_eq!(
            fetched.body["subscriber"]["sources"], sources,
            "case {source_name:?}: GetSubscriber must report the stored sources, got {:?}",
            fetched.body
        );
    }
}

/// RULE: a subscriber needs a data lake in the Region the request was signed for; without
/// one the call is refused `ResourceNotFoundException` (404).
#[tokio::test]
async fn creating_a_subscriber_without_a_data_lake_in_that_region_is_not_found() {
    let cases: Vec<(&str, Option<&str>, &str)> = vec![
        ("no data lake anywhere", None, "us-east-1"),
        ("a data lake in another Region", Some("eu-west-2"), "us-east-1"),
        ("a data lake in the caller's sibling Region", Some("us-east-1"), "us-west-2"),
    ];

    for (case, existing_region, calling_region) in &cases {
        let emulator = Emulator::new();
        if let Some(region) = existing_region {
            let reply = emulator
                .create_data_lake(region, json!({ "region": region }))
                .await;
            assert_eq!(reply.status, 200, "case {case:?}: setup failed with {}", reply.status);
        }

        let reply = emulator
            .create_subscriber(
                calling_region,
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                    "subscriberName": "sub-one",
                    "sources": [{ "awsLogSource": { "sourceName": "S3_DATA", "sourceVersion": "latest" } }],
                }),
            )
            .await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: wrong error code, got {:?}",
            reply.error_type
        );
    }
}

/// RULE: the store is BOUNDED. `CreateSubscriber` refuses (429 `ThrottlingException`) once
/// a Region already holds `MAX_SUBSCRIBERS_PER_REGION` subscribers; the bound is per
/// Region, and deleting one frees its slot.
#[tokio::test]
async fn the_subscriber_store_is_bounded_per_region() {
    use service_com_amazonaws_securitylake_securitylake::state::MAX_SUBSCRIBERS_PER_REGION;

    let emulator = Emulator::new();
    for region in ["us-east-1", "eu-west-2"] {
        let lake = emulator
            .create_data_lake(region, json!({ "region": region }))
            .await;
        assert_eq!(lake.status, 200, "setting up {region:?} got {}", lake.status);
    }

    let subscriber = |index: usize| {
        json!({
            "subscriberIdentity": { "principal": "123456789012", "externalId": format!("ext-{index}") },
            "subscriberName": format!("sub-{index}"),
            "sources": [{ "awsLogSource": { "sourceName": "VPC_FLOW", "sourceVersion": "latest" } }],
        })
    };

    let mut ids = Vec::new();
    for index in 0..MAX_SUBSCRIBERS_PER_REGION {
        let reply = emulator.create_subscriber("us-east-1", subscriber(index)).await;
        assert_eq!(
            reply.status, 200,
            "case \"subscriber {index}\": expected 200 up to the bound, got {} with {:?}",
            reply.status, reply.body
        );
        ids.push(text(&reply.body["subscriber"], "subscriberId", "fill").to_string());
    }

    // Rows: the call past the bound, then the same call after a delete frees a slot.
    let over = emulator
        .create_subscriber("us-east-1", subscriber(MAX_SUBSCRIBERS_PER_REGION))
        .await;
    assert_eq!(
        over.status, 429,
        "case \"past the bound\": expected 429, got {} with {:?}",
        over.status, over.body
    );
    assert_eq!(
        over.error_type.as_deref(),
        Some("ThrottlingException"),
        "case \"past the bound\": wrong error code, got {:?}",
        over.error_type
    );

    // The bound is PER REGION: another Region is unaffected.
    let other_region = emulator.create_subscriber("eu-west-2", subscriber(0)).await;
    assert_eq!(
        other_region.status, 200,
        "case \"another Region\": the bound must be per Region, got {} with {:?}",
        other_region.status, other_region.body
    );

    let deleted = emulator.delete_subscriber("us-east-1", &ids[0]).await;
    assert_eq!(
        deleted.status, 200,
        "case \"free a slot\": the delete must succeed, got {}",
        deleted.status
    );
    let again = emulator
        .create_subscriber("us-east-1", subscriber(MAX_SUBSCRIBERS_PER_REGION))
        .await;
    assert_eq!(
        again.status, 200,
        "case \"after a delete\": a freed slot must be usable, got {} with {:?}",
        again.status, again.body
    );
}

/// RULE: the data-lake store is BOUNDED too. `region` is a caller-supplied string, so
/// `CreateDataLake` refuses (429 `ThrottlingException`) once `MAX_DATA_LAKES` Regions are
/// held — while still UPDATING a Region it already holds, which adds no entry.
#[tokio::test]
async fn the_data_lake_store_is_bounded() {
    use service_com_amazonaws_securitylake_securitylake::state::MAX_DATA_LAKES;

    let emulator = Emulator::new();
    // `Region`'s pattern allows any number of digits, so these are all valid Region values
    // a client could send.
    for index in 1..=MAX_DATA_LAKES {
        let region = format!("us-east-{index}");
        let reply = emulator
            .create_data_lake(&region, json!({ "region": region }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {region:?}: expected 200 up to the bound, got {} with {:?}",
            reply.status, reply.body
        );
    }

    let over = format!("us-east-{}", MAX_DATA_LAKES + 1);
    let reply = emulator.create_data_lake(&over, json!({ "region": over })).await;
    assert_eq!(
        reply.status, 429,
        "case \"past the bound\": expected 429, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ThrottlingException"),
        "case \"past the bound\": wrong error code, got {:?}",
        reply.error_type
    );

    // An UPDATE of a Region already held adds no entry, so it is still accepted at the
    // bound.
    let update = emulator
        .create_data_lake(
            "us-east-1",
            json!({
                "region": "us-east-1",
                "encryptionConfiguration": { "kmsKeyId": "alias/rotated" },
            }),
        )
        .await;
    assert_eq!(
        update.status, 200,
        "case \"update at the bound\": expected 200, got {} with {:?}",
        update.status, update.body
    );

    // And a multi-Region request that would pass the bound is refused WHOLE: nothing is
    // added, not even the Region that would have fitted.
    let batch = emulator
        .send(
            "POST",
            "/v1/datalake",
            "us-east-1",
            Some(json!({
                "configurations": [{ "region": "eu-west-1" }, { "region": "eu-west-2" }],
                "metaStoreManagerRoleArn": META_STORE_ROLE,
            })),
        )
        .await;
    assert_eq!(
        batch.status, 429,
        "case \"batch past the bound\": expected 429, got {} with {:?}",
        batch.status, batch.body
    );
    let listed = emulator
        .list_data_lakes("us-east-1", "?regions=eu-west-1&regions=eu-west-2")
        .await;
    assert!(
        array(&listed, "dataLakes", "batch past the bound").is_empty(),
        "case \"batch past the bound\": a refused request must add nothing, got {:?}",
        listed.body
    );
}

/// RULE: a subscriber belongs to the Region it was created in, so only a caller of that
/// Region can read, page over, or delete it.
#[tokio::test]
async fn a_subscriber_is_visible_only_in_its_own_region() {
    let emulator = Emulator::new();
    // Rows: two Regions, each with its own data lake and its own subscriber.
    let regions = ["us-east-1", "eu-west-2"];
    let mut ids = Vec::new();
    for region in regions {
        let lake = emulator
            .create_data_lake(region, json!({ "region": region }))
            .await;
        assert_eq!(lake.status, 200, "setting up {region:?}: got {}", lake.status);
        let created = emulator
            .create_subscriber(
                region,
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                    "subscriberName": "sub-one",
                    "sources": [{ "awsLogSource": { "sourceName": "LAMBDA_EXECUTION", "sourceVersion": "latest" } }],
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "creating the {region:?} subscriber: got {} with {:?}",
            created.status, created.body
        );
        ids.push(text(&created.body["subscriber"], "subscriberId", region).to_string());
    }

    for (index, region) in regions.iter().enumerate() {
        let own = &ids[index];
        let other = &ids[1 - index];

        let listed = emulator.list_subscribers(region, "").await;
        let reported: Vec<String> = array(&listed, "subscribers", region)
            .iter()
            .map(|subscriber| text(subscriber, "subscriberId", region).to_string())
            .collect();
        assert_eq!(
            reported,
            vec![own.clone()],
            "case {region:?}: ListSubscribers must report only this Region's subscriber, got {:?}",
            listed.body
        );

        let fetched = emulator.get_subscriber(region, other).await;
        assert_eq!(
            fetched.status, 404,
            "case {region:?}: another Region's subscriber must not be readable, got {} with {:?}",
            fetched.status, fetched.body
        );

        let deleted = emulator.delete_subscriber(region, other).await;
        assert_eq!(
            deleted.status, 404,
            "case {region:?}: another Region's subscriber must not be deletable, got {}",
            deleted.status
        );
    }

    // And neither delete above removed anything.
    for (index, region) in regions.iter().enumerate() {
        let fetched = emulator.get_subscriber(region, &ids[index]).await;
        assert_eq!(
            fetched.status, 200,
            "case {region:?}: the subscriber must have survived the refused deletes, got {}",
            fetched.status
        );
    }
}

/// RULE: ListSubscribers pages over the stored subscribers in creation order, returns at
/// most `maxResults` of them, and hands back a `nextToken` only while more remain; a token
/// resumes exactly after the last item returned.
#[tokio::test]
async fn list_subscribers_pages_the_stored_order_for_every_page_size() {
    let emulator = Emulator::new();
    let created_lake = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(
        created_lake.status, 200,
        "setting up the data lake failed with {}",
        created_lake.status
    );

    let mut creation_order = Vec::new();
    for index in 0..5 {
        let reply = emulator
            .create_subscriber(
                "us-east-1",
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": format!("ext-{index}") },
                    "subscriberName": format!("sub-{index}"),
                    "sources": [{ "awsLogSource": { "sourceName": "ROUTE53", "sourceVersion": "2.0" } }],
                }),
            )
            .await;
        assert_eq!(reply.status, 200, "creating sub-{index}: got {}", reply.status);
        creation_order.push(text(&reply.body["subscriber"], "subscriberId", "setup").to_string());
    }

    // Rows differ in page size: every divisor, a non-divisor, the exact count, a size
    // past the count, and the `@range` maximum.
    let cases: Vec<(&str, Option<i32>)> = vec![
        ("one at a time", Some(1)),
        ("two at a time", Some(2)),
        ("three at a time", Some(3)),
        ("exactly the count", Some(5)),
        ("more than the count", Some(7)),
        ("the range maximum", Some(100)),
        ("the modelled default", None),
    ];

    for (case, page_size) in &cases {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut query = String::new();
            if let Some(size) = page_size {
                query.push_str(&format!("?maxResults={size}"));
            }
            if let Some(token) = &token {
                query.push(if query.is_empty() { '?' } else { '&' });
                query.push_str("nextToken=");
                query.push_str(token);
            }

            let reply = emulator.list_subscribers("us-east-1", &query).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} page {pages}: expected 200, got {} with {:?}",
                reply.status, reply.body
            );
            let page = array(&reply, "subscribers", case);
            let expected_len = page_size.unwrap_or(50).min(5) as usize;
            assert!(
                !page.is_empty() && page.len() <= expected_len,
                "case {case:?} page {pages}: expected 1..={expected_len} subscribers, got {}",
                page.len()
            );
            for subscriber in page {
                seen.push(text(subscriber, "subscriberId", case).to_string());
            }
            pages += 1;

            match reply.body["nextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
            assert!(pages <= 6, "case {case:?}: paging did not terminate");
        }

        assert_eq!(
            seen, creation_order,
            "case {case:?}: paging must visit every subscriber once, in creation order"
        );
        let expected_pages = 5_usize.div_ceil(page_size.unwrap_or(50).min(100) as usize);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages, got {pages}"
        );
    }
}

/// RULE: a `nextToken` whose anchor was deleted between pages still resumes — at the
/// position that anchor held — so a concurrent DeleteSubscriber cannot break an in-flight
/// paginator.
#[tokio::test]
async fn a_next_token_survives_the_deletion_of_its_anchor() {
    let emulator = Emulator::new();
    let created_lake = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(
        created_lake.status, 200,
        "setting up the data lake failed with {}",
        created_lake.status
    );

    let mut ids = Vec::new();
    for index in 0..3 {
        let reply = emulator
            .create_subscriber(
                "us-east-1",
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": format!("ext-{index}") },
                    "subscriberName": format!("sub-{index}"),
                    "sources": [{ "awsLogSource": { "sourceName": "WAF", "sourceVersion": "latest" } }],
                }),
            )
            .await;
        assert_eq!(reply.status, 200, "creating sub-{index}: got {}", reply.status);
        ids.push(text(&reply.body["subscriber"], "subscriberId", "setup").to_string());
    }

    let first = emulator.list_subscribers("us-east-1", "?maxResults=1").await;
    let token = first.body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"first page\": expected a nextToken, got {:?}", first.body))
        .to_string();

    // Rows: the anchor survives (the ordinary, exact resume); the anchor is deleted (resume
    // at the position it held); and an entry BEFORE the anchor is deleted too, where the
    // saved position is only a best effort and the handler documents the trade-off.
    let cases: Vec<(&str, Option<usize>, Vec<usize>)> = vec![
        ("anchor alive", None, vec![1, 2]),
        ("anchor deleted", Some(1), vec![2]),
        ("anchor and its predecessor deleted", Some(0), vec![]),
    ];

    for (case, delete_index, expected) in &cases {
        let emulator_state = emulator.list_subscribers("us-east-1", "").await;
        let before: Vec<String> = array(&emulator_state, "subscribers", case)
            .iter()
            .map(|subscriber| text(subscriber, "subscriberId", case).to_string())
            .collect();

        if let Some(index) = delete_index {
            let deleted = emulator.delete_subscriber("us-east-1", &ids[*index]).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: deleting subscriber {index} got {}",
                deleted.status
            );
        }

        let resumed = emulator
            .list_subscribers("us-east-1", &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {case:?}: expected 200, got {} with {:?} (store was {before:?})",
            resumed.status, resumed.body
        );
        let got: Vec<String> = array(&resumed, "subscribers", case)
            .iter()
            .map(|subscriber| text(subscriber, "subscriberId", case).to_string())
            .collect();
        let expected: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            got, expected,
            "case {case:?}: wrong resume, got {:?}",
            resumed.body
        );
    }
}

/// RULE: a `nextToken` this service did not issue is refused `BadRequestException` (400).
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_a_bad_request() {
    let emulator = Emulator::new();
    let created_lake = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(
        created_lake.status, 200,
        "setting up the data lake failed with {}",
        created_lake.status
    );
    let created = emulator
        .create_subscriber(
            "us-east-1",
            json!({
                "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                "subscriberName": "sub-one",
                "sources": [{ "awsLogSource": { "sourceName": "WAF", "sourceVersion": "latest" } }],
            }),
        )
        .await;
    assert_eq!(created.status, 200, "setup failed with {}", created.status);

    // Rows differ in HOW the token is wrong. They are BUILT here with the same encoding
    // the service uses (hex of the token payload), so the row says what the payload is.
    let unknown_id = "00000000-0000-0000-0000-000000000000";
    let cases: Vec<(&str, String)> = vec![
        ("not hex", "not-a-token".to_string()),
        ("empty", String::new()),
        ("odd-length hex", "abc".to_string()),
        ("hex of a non-token", hex::encode("nonsense")),
        // A well-formed id, but not the `<index>:<id>` form this service issues.
        ("hex of a bare id", hex::encode(unknown_id)),
        (
            "hex of a non-numeric index",
            hex::encode(format!("x:{unknown_id}")),
        ),
        (
            "hex of a negative index",
            hex::encode(format!("-1:{unknown_id}")),
        ),
        ("hex of an index with no id", hex::encode("3:")),
    ];

    for (case, token) in &cases {
        let reply = emulator
            .list_subscribers("us-east-1", &format!("?nextToken={token}"))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} with {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_type.as_deref(),
            Some("BadRequestException"),
            "case {case:?}: wrong error code, got {:?}",
            reply.error_type
        );
    }
}

/// RULE: DeleteSubscriber removes the stored subscriber — the next GetSubscriber is 404,
/// ListSubscribers no longer includes it (and keeps the order of the rest), and deleting
/// an id that is not stored is 404.
#[tokio::test]
async fn deleting_a_subscriber_is_observable_by_the_next_call() {
    let emulator = Emulator::new();
    let created_lake = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(
        created_lake.status, 200,
        "setting up the data lake failed with {}",
        created_lake.status
    );

    let mut ids = Vec::new();
    for index in 0..3 {
        let reply = emulator
            .create_subscriber(
                "us-east-1",
                json!({
                    "subscriberIdentity": { "principal": "123456789012", "externalId": format!("ext-{index}") },
                    "subscriberName": format!("sub-{index}"),
                    "sources": [{ "awsLogSource": { "sourceName": "SH_FINDINGS", "sourceVersion": "latest" } }],
                }),
            )
            .await;
        assert_eq!(reply.status, 200, "creating sub-{index}: got {}", reply.status);
        ids.push(text(&reply.body["subscriber"], "subscriberId", "setup").to_string());
    }

    // Rows: delete the middle one, then the first, then the last — the survivors must
    // keep their creation order at each step.
    let plan: Vec<(&str, usize)> = vec![("middle", 1), ("first", 0), ("last", 2)];
    let mut remaining = ids.clone();

    for (case, position) in &plan {
        let victim = ids[*position].clone();
        let reply = emulator.delete_subscriber("us-east-1", &victim).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );
        remaining.retain(|id| *id != victim);

        let fetched = emulator.get_subscriber("us-east-1", &victim).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: a deleted subscriber must be gone, got {} with {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: wrong error code, got {:?}",
            fetched.error_type
        );

        let listed = emulator.list_subscribers("us-east-1", "").await;
        let still_there: Vec<String> = array(&listed, "subscribers", case)
            .iter()
            .map(|subscriber| text(subscriber, "subscriberId", case).to_string())
            .collect();
        assert_eq!(
            still_there, remaining,
            "case {case:?}: ListSubscribers must report exactly the survivors, in order"
        );

        let again = emulator.delete_subscriber("us-east-1", &victim).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: deleting an absent subscriber is 404, got {} with {:?}",
            again.status, again.body
        );
    }
}

/// RULE: deleting a Region's data lake removes that Region's subscribers with it, so the
/// observable state stays coherent — a subscriber can never outlive the data lake it
/// subscribes to — while another Region's subscribers are untouched.
#[tokio::test]
async fn deleting_a_data_lake_takes_that_regions_subscribers_with_it() {
    // Rows: the Region whose data lake is deleted, and the Region that keeps its own.
    let cases: Vec<(&str, &str)> = vec![("us-east-1", "eu-west-2"), ("eu-west-2", "us-east-1")];

    for (deleted, kept) in &cases {
        let case = format!("delete {deleted}, keep {kept}");
        let emulator = Emulator::new();
        let mut ids = Vec::new();
        for region in [deleted, kept] {
            let lake = emulator
                .create_data_lake(region, json!({ "region": region }))
                .await;
            assert_eq!(
                lake.status, 200,
                "case {case:?}: setting up {region:?} got {}",
                lake.status
            );
            let created = emulator
                .create_subscriber(
                    region,
                    json!({
                        "subscriberIdentity": { "principal": "123456789012", "externalId": "ext-1" },
                        "subscriberName": "sub-one",
                        "sources": [{ "awsLogSource": { "sourceName": "CLOUD_TRAIL_MGMT", "sourceVersion": "1.0" } }],
                    }),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: creating the {region:?} subscriber got {} with {:?}",
                created.status, created.body
            );
            ids.push(text(&created.body["subscriber"], "subscriberId", &case).to_string());
        }

        let reply = emulator.delete_data_lake(deleted, &[deleted]).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );

        let gone = emulator.get_subscriber(deleted, &ids[0]).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: the deleted Region's subscriber must be gone, got {} with {:?}",
            gone.status, gone.body
        );
        let listed = emulator.list_subscribers(deleted, "").await;
        assert!(
            array(&listed, "subscribers", &case).is_empty(),
            "case {case:?}: the deleted Region must list no subscribers, got {:?}",
            listed.body
        );

        let survivor = emulator.get_subscriber(kept, &ids[1]).await;
        assert_eq!(
            survivor.status, 200,
            "case {case:?}: the kept Region's subscriber must survive, got {} with {:?}",
            survivor.status, survivor.body
        );
    }
}

/// RULE: DeleteDataLake removes the data lake of every Region it names, so ListDataLakes
/// no longer reports those Regions; a Region with no data lake is refused
/// `ResourceNotFoundException` (404) and nothing is removed.
#[tokio::test]
async fn deleting_data_lakes_removes_exactly_the_named_regions() {
    let all = ["us-east-1", "eu-west-2", "sa-east-1"];

    // Rows differ in how many Regions one call names.
    let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("one region", vec!["us-east-1"], vec!["eu-west-2", "sa-east-1"]),
        (
            "two regions",
            vec!["us-east-1", "sa-east-1"],
            vec!["eu-west-2"],
        ),
        ("every region", vec!["us-east-1", "eu-west-2", "sa-east-1"], vec![]),
    ];

    for (case, to_delete, expected) in &cases {
        let emulator = Emulator::new();
        for region in all {
            let reply = emulator
                .create_data_lake(region, json!({ "region": region }))
                .await;
            assert_eq!(reply.status, 200, "case {case:?}: setup of {region:?} got {}", reply.status);
        }

        let reply = emulator.delete_data_lake("us-east-1", to_delete).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} with {:?}",
            reply.status, reply.body
        );

        let listed = emulator.list_data_lakes("us-east-1", "").await;
        let expected: Vec<String> = expected.iter().map(|r| (*r).to_string()).collect();
        assert_eq!(
            reported_regions(&listed, case),
            expected,
            "case {case:?}: wrong regions left, got {:?}",
            listed.body
        );
    }

    // A Region with no data lake: refused, and the request applies nothing.
    let emulator = Emulator::new();
    let reply = emulator
        .create_data_lake("us-east-1", json!({ "region": "us-east-1" }))
        .await;
    assert_eq!(reply.status, 200, "setup got {}", reply.status);
    let reply = emulator
        .delete_data_lake("us-east-1", &["us-east-1", "ca-central-1"])
        .await;
    assert_eq!(
        reply.status, 404,
        "case \"an unknown region\": expected 404, got {} with {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"an unknown region\": wrong error code, got {:?}",
        reply.error_type
    );
    let listed = emulator.list_data_lakes("us-east-1", "").await;
    assert_eq!(
        reported_regions(&listed, "an unknown region"),
        vec!["us-east-1".to_string()],
        "case \"an unknown region\": a refused delete must remove nothing, got {:?}",
        listed.body
    );
}
