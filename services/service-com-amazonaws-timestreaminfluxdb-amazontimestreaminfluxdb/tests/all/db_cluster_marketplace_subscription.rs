// SPDX-License-Identifier: CC0-1.0

//! The AWS Marketplace subscription a cluster's deployment type is gated behind.
//!
//! An availability setting this service sells as an AWS Marketplace offering is
//! provisioned only for an account holding an ACTIVE subscription to that offering; a
//! request for one from an account without it is refused with the modelled
//! `ValidationException` (HTTP 400) and stores nothing. The read-replicas cluster
//! deployment type — the sole member of the modelled `ClusterDeploymentType` enum, so the
//! type of EVERY cluster `CreateDbCluster` makes — is such an offering.
//!
//! The rows below span the class rather than replaying one call: the deployment type is
//! named and left out, the name sits at both ends of its modelled length, the optional
//! members are absent and all present, a customer-created DB parameter group is applied,
//! and the Region changes (the subscription is the account's, not a Region's). Nothing in
//! this API subscribes an account, so the subscribed rows grant it through the store seam
//! the way AWS Marketplace does, out of band.

use serde_json::{json, Value};
use service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb as influxdb;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instance_lifecycle::{call, call_in, ok, router_and_engine, REGION};

/// The message AWS refuses a read-replicas create with while the account holds no active
/// AWS Marketplace subscription for Read Replicas — fixed text for this error class, with
/// nothing interpolated from the request or from the store.
const MISSING_SUBSCRIPTION_MESSAGE: &str = "Timestream for InfluxDB didn't find an active AWS \
    Marketplace subscription for Read Replicas associated with the account. You must \
    first create a subscription in Marketplace to access Read replicas availability \
    settings. You might need to wait up to 5 minutes for that subscription to be \
    active.";

/// A second Region, so a row can show the subscription is the ACCOUNT's: a create in a
/// Region the account never called before is refused just the same, and succeeds there
/// once the subscription is held.
const OTHER_REGION: &str = "eu-central-1";

/// The minimum a `CreateDbCluster` request must carry, per the model's `@required`
/// members (`name`, `dbInstanceType`, `vpcSubnetIds`, `vpcSecurityGroupIds`).
fn minimal_create(name: &str) -> Value {
    json!({
        "name": name,
        "dbInstanceType": "db.influx.medium",
        "vpcSubnetIds": ["subnet-aaa", "subnet-bbb"],
        "vpcSecurityGroupIds": ["sg-aaa"],
    })
}

/// One row: a well-formed `CreateDbCluster` request, and the Region it is signed into.
struct GatedCase {
    label: String,
    body: Value,
    region: &'static str,
}

/// The rows this suite spans. A row that needs a customer-created DB parameter group
/// creates it on `router` first — `CreateDbParameterGroup` needs no subscription, so it
/// is the positive control that the gate is about the gated deployment type alone.
async fn gated_cases(router: &MultiServiceRouter) -> Vec<GatedCase> {
    let group = ok(
        "setup: a customer parameter group",
        call(
            router,
            "CreateDbParameterGroup",
            json!({
                "name": "GatedClusterNodes",
                "parameters": {
                    "InfluxDBv3Enterprise": {
                        "ingestQueryInstances": 2,
                        "queryOnlyInstances": 1,
                        "dedicatedCompactor": false
                    }
                },
            }),
        )
        .await,
    );
    let group_id = group["id"]
        .as_str()
        .expect("the created parameter group reports its id")
        .to_string();

    vec![
        GatedCase {
            label: "only the required members: the deployment type left to the service"
                .to_string(),
            body: minimal_create("gated-minimal"),
            region: REGION,
        },
        GatedCase {
            label: "the deployment type named explicitly".to_string(),
            body: {
                let mut body = minimal_create("gated-explicit");
                body["deploymentType"] = json!("MULTI_NODE_READ_REPLICAS");
                body
            },
            region: REGION,
        },
        GatedCase {
            // The modelled `DbClusterName` length is 3..40.
            label: "a name at the modelled minimum length".to_string(),
            body: minimal_create("abc"),
            region: REGION,
        },
        GatedCase {
            label: "a name at the modelled maximum length".to_string(),
            body: minimal_create(&format!("z{}", "9".repeat(39))),
            region: REGION,
        },
        GatedCase {
            label: "every optional member the request can carry".to_string(),
            body: {
                let mut body = minimal_create("gated-full");
                body["deploymentType"] = json!("MULTI_NODE_READ_REPLICAS");
                body["username"] = json!("adminuser");
                body["password"] = json!("abcd1234EFGH");
                body["organization"] = json!("an-org");
                body["bucket"] = json!("a-bucket");
                // The ceilings of the modelled `port` (1024..65535) and
                // `allocatedStorage` (20..15360) ranges.
                body["port"] = json!(65535);
                body["allocatedStorage"] = json!(15360);
                body["dbStorageType"] = json!("InfluxIOIncludedT3");
                body["networkType"] = json!("DUAL");
                body["publiclyAccessible"] = json!(true);
                body["failoverMode"] = json!("NO_FAILOVER");
                body["tags"] = json!({ "team": "data" });
                body
            },
            region: REGION,
        },
        GatedCase {
            label: "a customer-created DB parameter group applied".to_string(),
            body: {
                let mut body = minimal_create("gated-with-group");
                body["dbParameterGroupIdentifier"] = json!(group_id);
                body
            },
            region: REGION,
        },
        GatedCase {
            label: format!("a create in {OTHER_REGION}: the subscription is the account's"),
            body: minimal_create("gated-elsewhere"),
            region: OTHER_REGION,
        },
    ]
}

/// Assert `answer` is the modelled refusal this error class carries.
fn assert_missing_subscription_refusal(case: &str, answer: (u16, Value)) {
    let (status, body) = answer;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    assert!(
        body["__type"]
            .as_str()
            .unwrap_or_default()
            .ends_with("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body}"
    );
    assert_eq!(
        body["message"].as_str(),
        Some(MISSING_SUBSCRIPTION_MESSAGE),
        "case {case:?}: unexpected message {:?}",
        body["message"]
    );
    // `ValidationException$reason` is `@required` and targets `ValidationExceptionReason`.
    let reason = body["reason"].as_str().unwrap_or_default();
    assert!(
        influxdb::ops::types::shared_types::VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
        "case {case:?}: {reason:?} is not a member of the modelled ValidationExceptionReason \
         enum, got {body}"
    );
}

#[tokio::test]
async fn a_create_of_a_gated_deployment_type_is_refused_without_a_marketplace_subscription() {
    let (router, engine) = router_and_engine();
    let cases = gated_cases(&router).await;

    for case in &cases {
        assert_missing_subscription_refusal(
            &case.label,
            call_in(&router, case.region, "CreateDbCluster", case.body.clone()).await,
        );
    }

    // The refusals stored nothing: no cluster, no claimed cluster name, and none of the
    // DB instances a cluster's nodes would have been.
    let (stored_clusters, claimed_names) = {
        let state = influxdb::state::service_state(&*engine)
            .expect("the shared engine holds this service's store");
        let store = state.lock();
        (
            store.db_clusters.keys().cloned().collect::<Vec<_>>(),
            store.db_cluster_names.keys().cloned().collect::<Vec<_>>(),
        )
    };
    assert!(
        stored_clusters.is_empty(),
        "case \"the refusals stored nothing\": expected no stored cluster, got \
         {stored_clusters:?}"
    );
    assert!(
        claimed_names.is_empty(),
        "case \"the refusals stored nothing\": expected no claimed cluster name, got \
         {claimed_names:?}"
    );

    for region in [REGION, OTHER_REGION] {
        let listed = ok(
            "the refusals stored nothing",
            call_in(&router, region, "ListDbInstances", json!({})).await,
        );
        assert_eq!(
            listed["items"].as_array().map(Vec::len),
            Some(0),
            "case \"the refusals stored nothing\" in {region}: a refused create must persist \
             no node, got {listed}"
        );
        // What the issue's caller sees: the cluster list stays empty after the refusal.
        let listed = ok(
            "the refusals stored nothing",
            call_in(&router, region, "ListDbClusters", json!({})).await,
        );
        assert_eq!(
            listed["items"].as_array().map(Vec::len),
            Some(0),
            "case \"the refusals stored nothing\" in {region}: the cluster list must stay \
             empty after a refusal, got {listed}"
        );
    }
}

#[tokio::test]
async fn the_same_create_is_served_once_the_account_holds_the_subscription() {
    let (router, engine) = router_and_engine();
    let cases = gated_cases(&router).await;

    // Refused first, so each row is shown to differ only in the account's subscription.
    for case in &cases {
        assert_missing_subscription_refusal(
            &case.label,
            call_in(&router, case.region, "CreateDbCluster", case.body.clone()).await,
        );
    }

    // The out-of-band grant: a buyer subscribes in AWS Marketplace, which no operation of
    // this service does.
    let granted = influxdb::state::subscribe_account_to_marketplace_offering(
        &*engine,
        influxdb::state::READ_REPLICAS_OFFERING,
    )
    .expect("the shared engine holds this service's store");
    assert!(
        granted,
        "case \"the out-of-band grant\": the account held no subscription before it"
    );

    for case in &cases {
        let label = &case.label;
        let created = ok(
            label,
            call_in(&router, case.region, "CreateDbCluster", case.body.clone()).await,
        );
        let id = created["dbClusterId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: expected a `dbClusterId`, got {created}"))
            .to_string();

        // Read it back: the cluster is of the gated deployment type, under the name the
        // refused request carried.
        let read = ok(
            label,
            call_in(
                &router,
                case.region,
                "GetDbCluster",
                json!({ "dbClusterId": &id }),
            )
            .await,
        );
        assert_eq!(
            read["name"], case.body["name"],
            "case {label:?}: the stored cluster must carry the requested name, got {read}"
        );
        assert_eq!(
            read["deploymentType"].as_str(),
            Some("MULTI_NODE_READ_REPLICAS"),
            "case {label:?}: unexpected deploymentType in {read}"
        );
    }

    // Every name the refused requests carried is now in the cluster list, the view the
    // issue's caller reconciles against.
    let listed = ok(
        "the subscribed creates are listed",
        call(&router, "ListDbClusters", json!({ "maxResults": 100 })).await,
    );
    let mut listed_names: Vec<String> = listed["items"]
        .as_array()
        .expect("an `items` list")
        .iter()
        .map(|item| item["name"].as_str().unwrap_or_default().to_string())
        .collect();
    listed_names.sort();
    let mut expected_names: Vec<String> = cases
        .iter()
        .map(|case| case.body["name"].as_str().unwrap_or_default().to_string())
        .collect();
    expected_names.sort();
    assert_eq!(
        listed_names, expected_names,
        "case \"the subscribed creates are listed\": expected exactly the created names, \
         got {listed}"
    );

    // The gate follows the store in both directions: a subscription cancelled in AWS
    // Marketplace puts the refusal back.
    let held = influxdb::state::unsubscribe_account_from_marketplace_offering(
        &*engine,
        influxdb::state::READ_REPLICAS_OFFERING,
    )
    .expect("the shared engine holds this service's store");
    assert!(
        held,
        "case \"the cancellation\": the account held the subscription before it"
    );
    assert_missing_subscription_refusal(
        "a create after the subscription was cancelled",
        call(&router, "CreateDbCluster", minimal_create("after-cancel")).await,
    );
}

#[test]
fn only_the_cluster_deployment_type_is_gated_behind_a_marketplace_offering() {
    use influxdb::ops::types::shared_types::{
        CLUSTER_DEPLOYMENT_TYPE_VALUES, DEPLOYMENT_TYPE_VALUES, RESOURCE_DEPLOYMENT_TYPE_VALUES,
        VALIDATION_EXCEPTION_REASON_VALUES,
    };
    use influxdb::state as st;

    // Every member of the modelled `ClusterDeploymentType` enum is an AWS Marketplace
    // offering of this service, including the one the issue did not name — the enum has
    // one member today, and the catalog is read from it rather than restated.
    for deployment_type in CLUSTER_DEPLOYMENT_TYPE_VALUES {
        let offering = st::marketplace_offering_for_deployment_type(deployment_type);
        assert!(
            offering.is_some(),
            "case {deployment_type:?}: a cluster deployment type must be gated behind an \
             offering, got {offering:?}"
        );
    }

    // The DB-instance deployment types (the modelled `DeploymentType` enum) come with the
    // service: nothing gates them.
    for deployment_type in DEPLOYMENT_TYPE_VALUES {
        let offering = st::marketplace_offering_for_deployment_type(deployment_type);
        assert!(
            offering.is_none(),
            "case {deployment_type:?}: an instance deployment type needs no subscription, \
             got {offering:?}"
        );
    }
    // `RestoreFromDbBackup` takes the wider `ResourceDeploymentType` enum, which spans
    // both: a member of it is gated exactly when it is a cluster deployment type, so the
    // same gate serves that operation when it is implemented.
    for deployment_type in RESOURCE_DEPLOYMENT_TYPE_VALUES {
        let gated = st::marketplace_offering_for_deployment_type(deployment_type).is_some();
        assert_eq!(
            gated,
            CLUSTER_DEPLOYMENT_TYPE_VALUES.contains(deployment_type),
            "case {deployment_type:?}: gated={gated}, but a resource deployment type is gated \
             exactly when it is a cluster deployment type"
        );
    }

    let unknown = st::marketplace_offering_for_deployment_type("NOT_A_DEPLOYMENT_TYPE");
    assert!(
        unknown.is_none(),
        "case \"NOT_A_DEPLOYMENT_TYPE\": expected no offering, got {unknown:?}"
    );

    assert!(
        VALIDATION_EXCEPTION_REASON_VALUES.contains(&st::VALIDATION_EXCEPTION_REASON_OTHER),
        "case {:?}: not a member of the modelled ValidationExceptionReason enum \
         {VALIDATION_EXCEPTION_REASON_VALUES:?}",
        st::VALIDATION_EXCEPTION_REASON_OTHER
    );
}
