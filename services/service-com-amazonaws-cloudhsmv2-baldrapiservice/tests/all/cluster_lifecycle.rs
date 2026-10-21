// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the cluster/HSM lifecycle this crate implements:
//! `CreateCluster`, `DescribeClusters`, `CreateHsm`, `DeleteHsm`, `DeleteCluster`.
//!
//! Every case drives the real wire path — `POST /` with
//! `X-Amz-Target: BaldrApiService.<Operation>` through the multi-service router — so
//! what is asserted is what a client sees, body and HTTP status included.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudhsmv2_baldrapiservice::state::{
    DELETED_CLUSTER_RETENTION, MAX_CLUSTERS, MAX_HSMS_PER_CLUSTER, TRANSITION_DELAY,
};
use service_com_amazonaws_cloudhsmv2_baldrapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying exactly this service, over one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// Call `operation` with `body`, signed for `region` so the service resolves the
    /// request's region the way it does in production (the SigV4 credential scope).
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("BaldrApiService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/cloudhsm/\
                     aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
                ),
            )
            .body(Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Call `operation` in the default test region.
    async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call_in(REGION, operation, body).await
    }

    /// Call `operation`, asserting a 200 and returning the response body.
    async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, response) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected {operation} to answer 200, got {status} {response}"
        );
        response
    }
}

/// The region most cases sign for.
const REGION: &str = "us-east-1";

/// A subnet id matching the modelled `^subnet-[0-9a-fA-F]{8,17}$`.
fn subnet(index: usize) -> String {
    format!("subnet-{:08x}{:09x}", 0x0abc_1234_u32, index)
}

/// Whether `value` is one of the cloudhsm ids modelled as
/// `^<prefix>[2-7a-zA-Z]{11,16}$`.
fn is_modelled_id(value: &str, prefix: &str) -> bool {
    let Some(body) = value.strip_prefix(prefix) else {
        return false;
    };
    (11..=16).contains(&body.len())
        && body
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || (b'2'..=b'7').contains(&b))
}

/// Whether `value` is one of the ids modelled as `^<prefix>[0-9a-fA-F]{8,17}$`.
fn is_hex_id(value: &str, prefix: &str) -> bool {
    let Some(body) = value.strip_prefix(prefix) else {
        return false;
    };
    (8..=17).contains(&body.len()) && body.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The `__type` an awsJson1_1 error body carries.
fn error_code(response: &Value) -> String {
    response
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// A cluster's `SubnetMapping` as a sorted AZ -> subnet map.
fn subnet_mapping(cluster: &Value) -> BTreeMap<String, String> {
    cluster
        .get("SubnetMapping")
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(az, subnet_id)| {
                    (
                        az.clone(),
                        subnet_id.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Wait past a clock-owed transition's due instant.
fn wait_for_transition() {
    std::thread::sleep(TRANSITION_DELAY + Duration::from_millis(400));
}

/// One `CreateCluster` request, and what the cluster it mints must report.
struct CreateCase {
    name: &'static str,
    region: &'static str,
    subnets: usize,
    hsm_type: &'static str,
    mode: Option<&'static str>,
    network_type: Option<&'static str>,
    retention: Option<(&'static str, &'static str)>,
    tags: Vec<(&'static str, &'static str)>,
    expected_mode: &'static str,
    expected_network_type: &'static str,
    expected_retention: (&'static str, &'static str),
}

/// The table: a `CreateCluster` answer is computed from the request and the service's
/// documented defaults, and `DescribeClusters` reports exactly the cluster that was
/// stored. Rows vary the region, the subnet count (the modelled `SubnetIds` bound is
/// `len 1..10`), the HSM type catalog, the mode, the network type, the retention bounds
/// and the tag count.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "defaults, one subnet",
            region: "us-east-1",
            subnets: 1,
            hsm_type: "hsm1.medium",
            mode: None,
            network_type: None,
            retention: None,
            tags: Vec::new(),
            expected_mode: "FIPS",
            expected_network_type: "IPV4",
            // CloudHSM's default retention window.
            expected_retention: ("DAYS", "90"),
        },
        CreateCase {
            name: "dualstack hsm2m across three subnets",
            region: "eu-west-2",
            subnets: 3,
            hsm_type: "hsm2m.medium",
            mode: Some("NON_FIPS"),
            network_type: Some("DUALSTACK"),
            // The published lower bound of `BackupRetentionPolicy$Value`.
            retention: Some(("DAYS", "7")),
            tags: vec![("Team", "crypto"), ("Env", "test")],
            expected_mode: "NON_FIPS",
            expected_network_type: "DUALSTACK",
            expected_retention: ("DAYS", "7"),
        },
        CreateCase {
            name: "gov region, the modelled maximum of ten subnets",
            region: "us-gov-west-1",
            subnets: 10,
            hsm_type: "hsm1.medium",
            mode: Some("FIPS"),
            network_type: Some("IPV4"),
            // The published upper bound of `BackupRetentionPolicy$Value`.
            retention: Some(("DAYS", "379")),
            tags: vec![("Owner", "sec")],
            expected_mode: "FIPS",
            expected_network_type: "IPV4",
            expected_retention: ("DAYS", "379"),
        },
    ]
}

impl CreateCase {
    /// This row as a `CreateCluster` request body.
    fn request(&self) -> Value {
        let subnets: Vec<String> = (0..self.subnets).map(subnet).collect();
        let mut body = json!({ "HsmType": self.hsm_type, "SubnetIds": subnets });
        if let Some(mode) = self.mode {
            body["Mode"] = json!(mode);
        }
        if let Some(network_type) = self.network_type {
            body["NetworkType"] = json!(network_type);
        }
        if let Some((retention_type, value)) = self.retention {
            body["BackupRetentionPolicy"] = json!({ "Type": retention_type, "Value": value });
        }
        if !self.tags.is_empty() {
            body["TagList"] = Value::Array(
                self.tags
                    .iter()
                    .map(|(key, value)| json!({ "Key": key, "Value": value }))
                    .collect(),
            );
        }
        body
    }

    /// The AZ -> subnet map this row's cluster must carry: one AZ per requested
    /// subnet, in the region the request was signed for.
    fn expected_subnet_mapping(&self) -> BTreeMap<String, String> {
        (0..self.subnets)
            .map(|index| {
                let letter = char::from(b'a' + index as u8);
                (format!("{}{letter}", self.region), subnet(index))
            })
            .collect()
    }
}

#[tokio::test]
async fn create_cluster_computes_the_cluster_and_describe_clusters_returns_it() {
    let emulator = Emulator::new();
    let mut minted: Vec<String> = Vec::new();

    for case in create_cases() {
        let name = case.name;
        let (status, response) = emulator
            .call_in(case.region, "CreateCluster", &case.request())
            .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected CreateCluster to answer 200, got {status} {response}"
        );
        let cluster = response
            .get("Cluster")
            .cloned()
            .unwrap_or_else(|| panic!("case {name:?}: no Cluster in {response}"));

        let cluster_id = cluster["ClusterId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&cluster_id, "cluster-"),
            "case {name:?}: expected an id matching ^cluster-[2-7a-zA-Z]{{11,16}}$, got \
             {cluster_id:?}"
        );
        assert!(
            !minted.contains(&cluster_id),
            "case {name:?}: expected a fresh cluster id, got the already-minted \
             {cluster_id:?}"
        );
        minted.push(cluster_id.clone());

        assert_eq!(
            cluster["HsmType"].as_str(),
            Some(case.hsm_type),
            "case {name:?}: expected the requested HsmType, got {:?}",
            cluster["HsmType"]
        );
        assert_eq!(
            cluster["State"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {name:?}: expected a cluster that is still being created, got {:?}",
            cluster["State"]
        );
        assert_eq!(
            cluster["BackupPolicy"].as_str(),
            Some("DEFAULT"),
            "case {name:?}: expected the only modelled BackupPolicy, got {:?}",
            cluster["BackupPolicy"]
        );
        assert_eq!(
            cluster["Mode"].as_str(),
            Some(case.expected_mode),
            "case {name:?}: expected Mode {:?}, got {:?}",
            case.expected_mode,
            cluster["Mode"]
        );
        assert_eq!(
            cluster["NetworkType"].as_str(),
            Some(case.expected_network_type),
            "case {name:?}: expected NetworkType {:?}, got {:?}",
            case.expected_network_type,
            cluster["NetworkType"]
        );
        assert_eq!(
            (
                cluster["BackupRetentionPolicy"]["Type"].as_str(),
                cluster["BackupRetentionPolicy"]["Value"].as_str()
            ),
            (
                Some(case.expected_retention.0),
                Some(case.expected_retention.1)
            ),
            "case {name:?}: expected retention {:?}, got {:?}",
            case.expected_retention,
            cluster["BackupRetentionPolicy"]
        );
        assert_eq!(
            subnet_mapping(&cluster),
            case.expected_subnet_mapping(),
            "case {name:?}: expected one Availability Zone per requested subnet"
        );
        assert_eq!(
            cluster["Hsms"].as_array().map(Vec::len),
            Some(0),
            "case {name:?}: expected a new cluster to carry no HSMs, got {:?}",
            cluster["Hsms"]
        );
        assert!(
            cluster["CreateTimestamp"].is_number(),
            "case {name:?}: expected a CreateTimestamp, got {:?}",
            cluster["CreateTimestamp"]
        );
        assert!(
            is_hex_id(cluster["VpcId"].as_str().unwrap_or_default(), "vpc-"),
            "case {name:?}: expected a minted VpcId, got {:?}",
            cluster["VpcId"]
        );
        assert!(
            is_hex_id(cluster["SecurityGroup"].as_str().unwrap_or_default(), "sg-"),
            "case {name:?}: expected a minted SecurityGroup, got {:?}",
            cluster["SecurityGroup"]
        );
        let expected_tags: Vec<Value> = case
            .tags
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        if expected_tags.is_empty() {
            assert!(
                cluster.get("TagList").is_none_or(Value::is_null),
                "case {name:?}: expected no TagList on an untagged cluster, got {:?}",
                cluster["TagList"]
            );
        } else {
            assert_eq!(
                cluster["TagList"].as_array(),
                Some(&expected_tags),
                "case {name:?}: expected the requested tags, got {:?}",
                cluster["TagList"]
            );
        }

        // The round trip: the stored cluster is what a describe returns.
        let described = emulator
            .ok(
                name,
                "DescribeClusters",
                &json!({ "Filters": { "clusterIds": [cluster_id] } }),
            )
            .await;
        let clusters = described["Clusters"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            clusters.len(),
            1,
            "case {name:?}: expected the created cluster to be described once, got \
             {described}"
        );
        let mut stored = clusters[0].clone();
        // The only member that may legitimately differ between the two answers is the
        // state, which the clock moves on its own.
        stored["State"] = cluster["State"].clone();
        assert_eq!(
            stored, cluster,
            "case {name:?}: expected DescribeClusters to return the cluster CreateCluster \
             computed"
        );
    }
}

#[tokio::test]
async fn an_hsm_is_visible_in_its_cluster_and_gone_after_it_is_deleted() {
    let emulator = Emulator::new();

    // One cluster over three subnets, so each HSM lands in a different Availability
    // Zone and each row below selects a different subnet from the stored mapping.
    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({
                "HsmType": "hsm2m.medium",
                "SubnetIds": [subnet(0), subnet(1), subnet(2)],
                "NetworkType": "DUALSTACK",
            }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();

    // Rows differ in the Availability Zone, in whether the caller chose the ENI
    // address, and in which of the three documented selectors deletes the HSM.
    let cases: Vec<(&str, usize, Option<&str>, &str)> = vec![
        ("zone a, address chosen by the service, deleted by HsmId", 0, None, "HsmId"),
        ("zone b, address chosen by the caller, deleted by EniIp", 1, Some("10.0.1.77"), "EniIp"),
        ("zone c, address chosen by the service, deleted by EniId", 2, None, "EniId"),
    ];

    for (name, zone_index, requested_ip, selector) in cases {
        let availability_zone = format!("{REGION}{}", char::from(b'a' + zone_index as u8));
        let mut request = json!({ "ClusterId": cluster_id, "AvailabilityZone": availability_zone });
        if let Some(ip) = requested_ip {
            request["IpAddress"] = json!(ip);
        }
        let created_hsm = emulator.ok(name, "CreateHsm", &request).await;
        let hsm = created_hsm["Hsm"].clone();

        let hsm_id = hsm["HsmId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_modelled_id(&hsm_id, "hsm-"),
            "case {name:?}: expected an id matching ^hsm-[2-7a-zA-Z]{{11,16}}$, got {hsm_id:?}"
        );
        let eni_id = hsm["EniId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_hex_id(&eni_id, "eni-"),
            "case {name:?}: expected an id matching ^eni-[0-9a-fA-F]{{8,17}}$, got {eni_id:?}"
        );
        assert_eq!(
            hsm["SubnetId"].as_str(),
            Some(subnet(zone_index).as_str()),
            "case {name:?}: expected the cluster's subnet for {availability_zone}, got {:?}",
            hsm["SubnetId"]
        );
        assert_eq!(
            hsm["HsmType"].as_str(),
            Some("hsm2m.medium"),
            "case {name:?}: expected the cluster's HSM type, got {:?}",
            hsm["HsmType"]
        );
        assert_eq!(
            hsm["State"].as_str(),
            Some("CREATE_IN_PROGRESS"),
            "case {name:?}: expected a new HSM to still be creating, got {:?}",
            hsm["State"]
        );
        let eni_ip = hsm["EniIp"].as_str().unwrap_or_default().to_string();
        match requested_ip {
            Some(ip) => assert_eq!(
                eni_ip, ip,
                "case {name:?}: expected the requested IpAddress, got {eni_ip:?}"
            ),
            None => assert!(
                eni_ip.starts_with(&format!("10.0.{zone_index}.")),
                "case {name:?}: expected an address from that zone's range, got {eni_ip:?}"
            ),
        }
        assert!(
            hsm["EniIpV6"].as_str().is_some_and(|v6| v6.contains(':')),
            "case {name:?}: expected a DUALSTACK cluster's HSM to carry an IPv6 address, \
             got {:?}",
            hsm["EniIpV6"]
        );

        // The HSM is visible inside its cluster.
        let described = emulator
            .ok(name, "DescribeClusters", &json!({}))
            .await;
        let listed = described["Clusters"][0]["Hsms"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            listed.iter().any(|listed| listed["HsmId"] == hsm["HsmId"]),
            "case {name:?}: expected {hsm_id} in the cluster's Hsms, got {listed:?}"
        );

        // Deleted by whichever selector this row names.
        let mut delete = json!({ "ClusterId": cluster_id });
        delete[selector] = match selector {
            "HsmId" => json!(hsm_id),
            "EniId" => json!(eni_id),
            _ => json!(eni_ip),
        };
        let deleted = emulator.ok(name, "DeleteHsm", &delete).await;
        assert_eq!(
            deleted["HsmId"].as_str(),
            Some(hsm_id.as_str()),
            "case {name:?}: expected DeleteHsm to report the deleted HSM, got {deleted}"
        );

        // Once the deletion's span has elapsed, the HSM is gone from the cluster.
        wait_for_transition();
        let described = emulator
            .ok(name, "DescribeClusters", &json!({}))
            .await;
        let remaining = described["Clusters"][0]["Hsms"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            !remaining.iter().any(|listed| listed["HsmId"] == hsm["HsmId"]),
            "case {name:?}: expected {hsm_id} to be gone, got {remaining:?}"
        );

        // Deleting it a second time finds nothing.
        let (status, response) = emulator.call("DeleteHsm", &delete).await;
        assert_eq!(
            (status, error_code(&response).contains("CloudHsmResourceNotFoundException")),
            (400, true),
            "case {name:?}: expected a second delete to be refused as not found, got \
             {status} {response}"
        );
    }
}

#[tokio::test]
async fn every_transition_this_emulator_owes_comes_due_on_its_own() {
    let emulator = Emulator::new();

    let created = emulator
        .ok(
            "cluster",
            "CreateCluster",
            &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0)] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    assert_eq!(
        created["Cluster"]["State"].as_str(),
        Some("CREATE_IN_PROGRESS"),
        "case \"cluster\": expected the create answer to show the transient state, got {:?}",
        created["Cluster"]["State"]
    );

    wait_for_transition();
    let described = emulator
        .ok("cluster", "DescribeClusters", &json!({}))
        .await;
    assert_eq!(
        described["Clusters"][0]["State"].as_str(),
        Some("UNINITIALIZED"),
        "case \"cluster\": expected a created cluster to settle at UNINITIALIZED, got {described}"
    );

    let hsm = emulator
        .ok(
            "hsm",
            "CreateHsm",
            &json!({ "ClusterId": cluster_id, "AvailabilityZone": format!("{REGION}a") }),
        )
        .await;
    assert_eq!(
        hsm["Hsm"]["State"].as_str(),
        Some("CREATE_IN_PROGRESS"),
        "case \"hsm\": expected the create answer to show the transient state, got {hsm}"
    );
    let hsm_id = hsm["Hsm"]["HsmId"].as_str().expect("an hsm id").to_string();

    wait_for_transition();
    let described = emulator
        .ok("hsm", "DescribeClusters", &json!({}))
        .await;
    assert_eq!(
        described["Clusters"][0]["Hsms"][0]["State"].as_str(),
        Some("ACTIVE"),
        "case \"hsm\": expected a created HSM to settle at ACTIVE, got {described}"
    );

    // A cluster that still contains an HSM cannot be deleted.
    let (status, response) = emulator
        .call("DeleteCluster", &json!({ "ClusterId": cluster_id }))
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmInvalidRequestException")),
        (400, true),
        "case \"cluster with an HSM\": expected the delete to be refused, got {status} {response}"
    );

    emulator
        .ok(
            "hsm",
            "DeleteHsm",
            &json!({ "ClusterId": cluster_id, "HsmId": hsm_id }),
        )
        .await;
    wait_for_transition();

    let deleted = emulator
        .ok("cluster", "DeleteCluster", &json!({ "ClusterId": cluster_id }))
        .await;
    assert_eq!(
        deleted["Cluster"]["State"].as_str(),
        Some("DELETE_IN_PROGRESS"),
        "case \"cluster\": expected the delete answer to show the transient state, got {deleted}"
    );

    wait_for_transition();
    let described = emulator
        .ok("cluster", "DescribeClusters", &json!({}))
        .await;
    assert_eq!(
        described["Clusters"][0]["State"].as_str(),
        Some("DELETED"),
        "case \"cluster\": expected a deleted cluster to settle at DELETED, got {described}"
    );


    // And it is no longer a resource another operation can act on.
    for (operation, body) in [
        (
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": format!("{REGION}a") }),
        ),
        ("DeleteCluster", json!({ "ClusterId": cluster_id })),
        (
            "DeleteHsm",
            json!({ "ClusterId": cluster_id, "HsmId": hsm_id }),
        ),
    ] {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            (status, error_code(&response).contains("CloudHsmResourceNotFoundException")),
            (400, true),
            "case {operation:?} on a deleted cluster: expected 400 \
             CloudHsmResourceNotFoundException, got {status} {response}"
        );
    }

    // Once the retention window on the terminal state has passed, the store reclaims
    // the record instead of holding it for the life of the process.
    std::thread::sleep(DELETED_CLUSTER_RETENTION + Duration::from_millis(400));
    let described = emulator
        .ok("cluster", "DescribeClusters", &json!({}))
        .await;
    assert_eq!(
        described["Clusters"].as_array().map(Vec::len),
        Some(0),
        "case \"cluster\": expected the deleted cluster to be reclaimed, got {described}"
    );
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_the_ceilings_it_keeps_on_itself() {
    // Clusters: the store holds MAX_CLUSTERS of them and refuses the next create.
    let emulator = Emulator::new();
    for index in 0..MAX_CLUSTERS {
        emulator
            .ok(
                &format!("cluster {index}"),
                "CreateCluster",
                &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0)] }),
            )
            .await;
    }
    let (status, response) = emulator
        .call(
            "CreateCluster",
            &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0)] }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmServiceException")),
        (400, true),
        "case \"one cluster past the ceiling\": expected 400 CloudHsmServiceException, got \
         {status} {response}"
    );

    // HSMs: one cluster holds MAX_HSMS_PER_CLUSTER of them — including the ones whose
    // address the caller chose, which is the path that bypasses address exhaustion —
    // and refuses the next create.
    let emulator = Emulator::new();
    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0)] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    let availability_zone = format!("{REGION}a");
    for index in 0..MAX_HSMS_PER_CLUSTER {
        let mut request =
            json!({ "ClusterId": cluster_id, "AvailabilityZone": availability_zone });
        // Half the rows let the service choose the address, half choose it themselves.
        if index % 2 == 0 {
            request["IpAddress"] = json!(format!("10.1.0.{}", index + 4));
        }
        emulator
            .ok(&format!("hsm {index}"), "CreateHsm", &request)
            .await;
    }
    let (status, response) = emulator
        .call(
            "CreateHsm",
            &json!({
                "ClusterId": cluster_id,
                "AvailabilityZone": availability_zone,
                "IpAddress": "10.2.0.9",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).contains("CloudHsmServiceException")),
        (400, true),
        "case \"one HSM past the ceiling\": expected 400 CloudHsmServiceException, got \
         {status} {response}"
    );
}

#[tokio::test]
async fn describe_clusters_pages_by_max_results_and_round_trips_next_token() {
    let emulator = Emulator::new();

    let mut created: Vec<String> = Vec::new();
    for index in 0..7 {
        let response = emulator
            .ok(
                "fixture",
                "CreateCluster",
                &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(index)] }),
            )
            .await;
        created.push(
            response["Cluster"]["ClusterId"]
                .as_str()
                .expect("a cluster id")
                .to_string(),
        );
    }
    created.sort();

    // Rows vary the page size across the modelled `range 1..25`: both bounds and two
    // sizes that do not divide the cluster count.
    for page_size in [1_usize, 2, 3, 25] {
        let name = format!("MaxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0;

        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let response = emulator.ok(&name, "DescribeClusters", &request).await;
            let page = response["Clusters"].as_array().cloned().unwrap_or_default();
            assert!(
                page.len() <= page_size,
                "case {name:?}: expected at most {page_size} clusters in a page, got {}",
                page.len()
            );
            for cluster in &page {
                seen.push(
                    cluster["ClusterId"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            next_token = response["NextToken"].as_str().map(str::to_string);
            pages += 1;
            assert!(
                pages <= 10,
                "case {name:?}: expected paging to terminate, got {pages} pages"
            );
            if next_token.is_none() {
                break;
            }
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {name:?}: expected no cluster on two pages, got {seen:?}"
        );
        let mut sorted = seen.clone();
        sorted.sort();
        assert_eq!(
            sorted, created,
            "case {name:?}: expected paging to report every stored cluster exactly once"
        );
        assert_eq!(
            pages,
            created.len().div_ceil(page_size),
            "case {name:?}: expected {} pages for {} clusters",
            created.len().div_ceil(page_size),
            created.len()
        );
    }
}

#[tokio::test]
async fn requests_outside_the_documented_rules_are_refused() {
    let emulator = Emulator::new();

    let created = emulator
        .ok(
            "fixture",
            "CreateCluster",
            &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0), subnet(1)] }),
        )
        .await;
    let cluster_id = created["Cluster"]["ClusterId"]
        .as_str()
        .expect("a cluster id")
        .to_string();
    let unknown_cluster = "cluster-aaaaaaaaaaa2";
    let unknown_hsm = "hsm-aaaaaaaaaaa2";

    // Each row: the rule, the request that breaks it, and the modelled error it must
    // answer with. Every one is a 400.
    let cases: Vec<(&str, &str, Value, &str)> = vec![
        (
            "an HSM type outside the documented catalog",
            "CreateCluster",
            json!({ "HsmType": "hsm9.large", "SubnetIds": [subnet(0)] }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "the same subnet named twice",
            "CreateCluster",
            json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(0), subnet(0)] }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention window one day below the published range",
            "CreateCluster",
            json!({
                "HsmType": "hsm1.medium",
                "SubnetIds": [subnet(0)],
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "6" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention window one day above the published range",
            "CreateCluster",
            json!({
                "HsmType": "hsm1.medium",
                "SubnetIds": [subnet(0)],
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "380" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a retention policy missing its Value",
            "CreateCluster",
            json!({
                "HsmType": "hsm1.medium",
                "SubnetIds": [subnet(0)],
                "BackupRetentionPolicy": { "Type": "DAYS" },
            }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a source backup this emulator does not hold",
            "CreateCluster",
            json!({
                "HsmType": "hsm1.medium",
                "SubnetIds": [subnet(0)],
                "SourceBackupId": "backup-aaaaaaaaaaa2",
            }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "an HSM in a cluster that does not exist",
            "CreateHsm",
            json!({ "ClusterId": unknown_cluster, "AvailabilityZone": format!("{REGION}a") }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "an HSM in a zone the cluster has no subnet in",
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": format!("{REGION}f") }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "an HSM in another region's zone",
            "CreateHsm",
            json!({ "ClusterId": cluster_id, "AvailabilityZone": "eu-west-2a" }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a delete naming no HSM at all",
            "DeleteHsm",
            json!({ "ClusterId": cluster_id }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a delete naming an HSM the cluster does not contain",
            "DeleteHsm",
            json!({ "ClusterId": cluster_id, "HsmId": unknown_hsm }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a delete in a cluster that does not exist",
            "DeleteHsm",
            json!({ "ClusterId": unknown_cluster, "HsmId": unknown_hsm }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a cluster delete naming a cluster that does not exist",
            "DeleteCluster",
            json!({ "ClusterId": unknown_cluster }),
            "CloudHsmResourceNotFoundException",
        ),
        (
            "a filter field the operation does not document",
            "DescribeClusters",
            json!({ "Filters": { "clusterNames": ["anything"] } }),
            "CloudHsmInvalidRequestException",
        ),
        (
            "a NextToken this emulator never minted",
            "DescribeClusters",
            json!({ "NextToken": "not-a-token" }),
            "CloudHsmInvalidRequestException",
        ),
    ];

    for (name, operation, body, expected) in cases {
        let (status, response) = emulator.call(operation, &body).await;
        let code = error_code(&response);
        assert_eq!(
            (status, code.contains(expected)),
            (400, true),
            "case {name:?}: expected {operation} to answer 400 {expected}, got {status} \
             {response}"
        );
    }
}

#[tokio::test]
async fn describe_clusters_filters_on_every_documented_field() {
    let emulator = Emulator::new();

    let mut clusters: Vec<Value> = Vec::new();
    for index in 0..3 {
        let response = emulator
            .ok(
                "fixture",
                "CreateCluster",
                &json!({ "HsmType": "hsm1.medium", "SubnetIds": [subnet(index)] }),
            )
            .await;
        clusters.push(response["Cluster"].clone());
    }
    // Every cluster has settled, so `states` selects on the state they now report.
    wait_for_transition();

    let first_id = clusters[0]["ClusterId"].as_str().unwrap_or_default();
    let second_vpc = clusters[1]["VpcId"].as_str().unwrap_or_default();

    let cases: Vec<(&str, Value, usize)> = vec![
        ("clusterIds", json!({ "clusterIds": [first_id] }), 1),
        ("vpcIds", json!({ "vpcIds": [second_vpc] }), 1),
        ("states", json!({ "states": ["UNINITIALIZED"] }), 3),
        ("states, none matching", json!({ "states": ["ACTIVE"] }), 0),
        (
            "clusterIds and states together",
            json!({ "clusterIds": [first_id], "states": ["ACTIVE"] }),
            0,
        ),
        (
            "an id no cluster carries",
            json!({ "clusterIds": ["cluster-aaaaaaaaaaa2"] }),
            0,
        ),
    ];

    for (name, filters, expected) in cases {
        let response = emulator
            .ok(name, "DescribeClusters", &json!({ "Filters": filters }))
            .await;
        let returned = response["Clusters"].as_array().map(Vec::len).unwrap_or(0);
        assert_eq!(
            returned, expected,
            "case {name:?}: expected {expected} clusters, got {returned} in {response}"
        );
    }
}

#[tokio::test]
async fn operations_outside_this_slice_still_answer_501() {
    let emulator = Emulator::new();

    for (operation, body) in [
        (
            "InitializeCluster",
            json!({
                "ClusterId": "cluster-aaaaaaaaaaa2",
                "SignedCert": "c2lnbmVk",
                "TrustAnchor": "YW5jaG9y",
            }),
        ),
        (
            "ModifyCluster",
            json!({
                "ClusterId": "cluster-aaaaaaaaaaa2",
                "BackupRetentionPolicy": { "Type": "DAYS", "Value": "90" },
            }),
        ),
        ("DescribeBackups", json!({})),
        ("ListTags", json!({ "ResourceId": "cluster-aaaaaaaaaaa2" })),
    ] {
        let (status, response) = emulator.call(operation, &body).await;
        assert_eq!(
            status, 501,
            "case {operation:?}: expected a scaffolded operation to answer 501, got \
             {status} {response}"
        );
    }
}
