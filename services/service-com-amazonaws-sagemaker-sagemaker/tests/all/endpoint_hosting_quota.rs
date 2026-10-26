// SPDX-License-Identifier: CC0-1.0

//! End-to-end wire tests for the hosting capacity `CreateEndpoint` admits a deployment
//! within.
//!
//! Every case goes over the wire (`POST /` with `X-Amz-Target: SageMaker.<Op>`,
//! `awsJson1_1`) through the same `MultiServiceRouter` the production router uses, reusing
//! the harness in [`crate::inference_deployment_lifecycle`], so what these tests exercise
//! is what a real SDK client gets.
//!
//! The tests assert RULES over TABLES of inputs rather than one recorded call:
//!
//! * an endpoint is admitted only when the account's quota for each ML instance type its
//!   configuration's variants name covers the instances they ask for, and a deployment
//!   past one is refused with the modelled resource-limit failure, naming that quota, its
//!   limit, the account's utilisation and the request's delta — and creating nothing;
//! * the utilisation is read off the endpoints that exist, so what one endpoint holds is
//!   unavailable to the next and deleting it gives the capacity back.

use serde_json::{json, Value};

use crate::inference_deployment_lifecycle::{
    error_code, message, primary_container, str_field, Emulator,
};

/// The emulator's per-instance-type bound for a type whose published default is not zero.
const DEFAULT_QUOTA: i64 =
    service_com_amazonaws_sagemaker_sagemaker::state::DEFAULT_ENDPOINT_USAGE_QUOTA;

/// A one-variant endpoint-configuration body asking for `instances` instances of
/// `instance_type`.
fn config_body(name: &str, model: &str, instance_type: &str, instances: i64) -> Value {
    json!({
        "EndpointConfigName": name,
        "ProductionVariants": [{
            "VariantName": "AllTraffic",
            "ModelName": model,
            "InitialInstanceCount": instances,
            "InstanceType": instance_type,
        }],
    })
}

/// How SageMaker words a request past an account quota it publishes, as
/// `state::account_quota_exceeded` states it for the endpoint-usage quota of
/// `instance_type`.
fn over_quota(instance_type: &str, limit: i64, in_use: i64, delta: i64) -> String {
    format!(
        "The account-level service limit '{instance_type} for endpoint usage' is {limit} \
         Instances, with current utilization of {in_use} Instances and a request delta of \
         {delta} Instances. Please use AWS Service Quotas to request an increase for this \
         quota. If AWS Service Quotas is not available, contact AWS support to request an \
         increase for this quota."
    )
}

/// A model every configuration below hosts, created through the implemented `CreateModel`.
async fn host_model(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "CreateModel",
            &json!({ "ModelName": name, "PrimaryContainer": primary_container("demo:1") }),
        )
        .await;
}

/// RULE: `CreateEndpoint` admits a deployment only when the account's endpoint-usage quota
/// for every ML instance type the configuration's variants name covers the instances they
/// ask for; a deployment past one is refused `ResourceLimitExceeded` (HTTP 400) naming that
/// quota, its limit, the utilisation and the delta, and no endpoint is created.
#[tokio::test]
async fn an_endpoint_is_admitted_only_within_the_hosting_quota_of_every_instance_type() {
    // Rows differ along the instance family (the accelerated generations an account is
    // granted no hosting capacity on by default, an older accelerated one, and general
    // purpose/compute types), the size within a family, and the instance count — including
    // one past the bound for a type that HAS capacity.
    // Source for every value: the modelled hosting value set,
    // `smithy_query(query="shape",shape="ProductionVariantInstanceType")`.
    let rows: [(&str, &str, i64, Option<i64>); 9] = [
        ("a zero-capacity GPU generation", "ml.p4de.24xlarge", 1, Some(0)),
        ("another size of that generation", "ml.p4d.24xlarge", 2, Some(0)),
        ("the next GPU generation", "ml.p5.48xlarge", 1, Some(0)),
        ("a small size of that generation", "ml.p5.4xlarge", 4, Some(0)),
        ("the newest GPU generation", "ml.p6-b200.48xlarge", 1, Some(0)),
        ("a Trainium generation", "ml.trn1.32xlarge", 3, Some(0)),
        ("an older accelerated generation", "ml.p3.2xlarge", 1, None),
        ("a general purpose type", "ml.m5.large", 1, None),
        (
            "a compute type one instance past the bound",
            "ml.c5.xlarge",
            DEFAULT_QUOTA + 1,
            Some(DEFAULT_QUOTA),
        ),
    ];

    for (case, instance_type, instances, refused_at) in rows {
        let emulator = Emulator::new();
        host_model(&emulator, "quota-model").await;
        emulator
            .ok(
                "CreateEndpointConfig",
                &config_body("quota-cfg", "quota-model", instance_type, instances),
            )
            .await;

        let (status, answered) = emulator
            .call(
                "CreateEndpoint",
                &json!({ "EndpointName": "quota-ep", "EndpointConfigName": "quota-cfg" }),
            )
            .await;

        match refused_at {
            Some(limit) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, "ResourceLimitExceeded"),
                    "case {case:?}: expected 400 ResourceLimitExceeded, got {status} \
                     {answered:?}"
                );
                assert_eq!(
                    message(&answered),
                    over_quota(instance_type, limit, 0, instances),
                    "case {case:?}: expected the quota, the limit, the utilisation and the \
                     delta, got {:?}",
                    message(&answered)
                );
                // The refused deployment created nothing: the endpoint cannot be read back.
                let (read_status, read) = emulator
                    .call("DescribeEndpoint", &json!({ "EndpointName": "quota-ep" }))
                    .await;
                assert_eq!(
                    (read_status, message(&read)),
                    (400, "Could not find endpoint \"quota-ep\".".to_string()),
                    "case {case:?}: a refused deployment must create no endpoint, got \
                     {read_status} {read:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the deployment to be admitted, got {status} \
                     {answered:?}"
                );
                let described = emulator
                    .ok("DescribeEndpoint", &json!({ "EndpointName": "quota-ep" }))
                    .await;
                assert_eq!(
                    str_field(&described, "EndpointArn"),
                    str_field(&answered, "EndpointArn"),
                    "case {case:?}: the admitted endpoint must read back, got {described:?}"
                );
            }
        }
    }
}

/// RULE: the utilisation a hosting quota is measured against is what the account's
/// endpoints HOLD — so capacity one endpoint took is unavailable to the next, a deployment
/// that exactly fills the quota is still admitted, and deleting an endpoint gives its
/// capacity back.
#[tokio::test]
async fn hosting_capacity_is_counted_over_the_endpoints_that_exist() {
    let emulator = Emulator::new();
    let instance_type = "ml.m5.xlarge";
    host_model(&emulator, "usage-model").await;

    // Three configurations of the same type: one just short of the quota, one of two
    // instances and one of a single instance.
    for (name, instances) in [
        ("usage-most", DEFAULT_QUOTA - 1),
        ("usage-two", 2),
        ("usage-one", 1),
    ] {
        emulator
            .ok(
                "CreateEndpointConfig",
                &config_body(name, "usage-model", instance_type, instances),
            )
            .await;
    }

    // The first deployment fits: nothing is in use yet.
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "usage-first", "EndpointConfigName": "usage-most" }),
        )
        .await;

    // Two more instances no longer fit, and the refusal counts what the first holds.
    let (status, refused) = emulator
        .call(
            "CreateEndpoint",
            &json!({ "EndpointName": "usage-second", "EndpointConfigName": "usage-two" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str(), message(&refused)),
        (
            400,
            "ResourceLimitExceeded",
            over_quota(instance_type, DEFAULT_QUOTA, DEFAULT_QUOTA - 1, 2)
        ),
        "case \"two instances past the quota\": expected the utilisation of the endpoint \
         that exists, got {status} {refused:?}"
    );

    // One instance exactly fills the quota, so it is admitted.
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "usage-exact", "EndpointConfigName": "usage-one" }),
        )
        .await;

    // With the quota full, even one more instance is refused.
    let (status, full) = emulator
        .call(
            "CreateEndpoint",
            &json!({ "EndpointName": "usage-overflow", "EndpointConfigName": "usage-one" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&full).as_str(), message(&full)),
        (
            400,
            "ResourceLimitExceeded",
            over_quota(instance_type, DEFAULT_QUOTA, DEFAULT_QUOTA, 1)
        ),
        "case \"one instance past a full quota\": expected the full utilisation, got \
         {status} {full:?}"
    );

    // Deleting the endpoint that holds the capacity gives it back, so the same request is
    // admitted afterwards.
    emulator
        .ok("DeleteEndpoint", &json!({ "EndpointName": "usage-first" }))
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({ "EndpointName": "usage-overflow", "EndpointConfigName": "usage-two" }),
        )
        .await;

    // A serverless variant holds no instances of any type, so it is admitted whatever the
    // account already runs.
    emulator
        .ok(
            "CreateEndpointConfig",
            &json!({
                "EndpointConfigName": "usage-serverless",
                "ProductionVariants": [{
                    "VariantName": "AllTraffic",
                    "ModelName": "usage-model",
                    "ServerlessConfig": { "MemorySizeInMB": 1024, "MaxConcurrency": 1 },
                }],
            }),
        )
        .await;
    emulator
        .ok(
            "CreateEndpoint",
            &json!({
                "EndpointName": "usage-serverless-ep",
                "EndpointConfigName": "usage-serverless",
            }),
        )
        .await;
}
