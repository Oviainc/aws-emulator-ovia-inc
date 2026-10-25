// SPDX-License-Identifier: CC0-1.0

//! `ListDeviceResources`: the resources available on one managed device, end to end
//! over HTTP.
//!
//! Every case goes through the router, so it exercises routing, deserialization, the
//! handler, the persisted store and serialization — the same path a client takes.
//!
//! The rules under test:
//!
//! 1. the answer is the projection of the addressed device's OWN resources — its Amazon
//!    EC2 instances, the only supported resource type — in the cache's own order, each
//!    reported with that type, the resource's id and its ARN; a device that runs none
//!    answers an empty page, and a resource of one device is never visible through
//!    another;
//! 2. `type` FILTERS the answer by resource type: a type the device's resources carry
//!    selects them all, any other value selects none of them, and neither is a refusal;
//! 3. `maxResults`/`nextToken` PARTITION one device's resources: walking the pages
//!    concatenates to the unpaged answer exactly once, and the last page carries no
//!    token;
//! 4. a `managedDeviceId` that names no device of the account in the signed Region is
//!    refused with the modelled HTTP 404 `ResourceNotFoundException`, whatever page or
//!    filter was asked of it, and a `nextToken` this service never issued is the
//!    modelled HTTP 400 `ValidationException`;
//! 5. the operation is a report: two identical calls answer identically and a call
//!    changes nothing a later caller sees.
//!
//! Each case is a TABLE whose rows differ along what the rule generalises over — the
//! device, its resource count, the filter value, and the page size.
//!
//! The tiny HTTP harness below is local to this suite on purpose: a sibling suite's
//! helpers are private to its module, and this file must keep compiling while that one
//! changes.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::{
    device_instance_ids, managed_device_id, service_state,
};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request in this suite signs for, which is also the Region its
/// devices are activated in.
const SIGNED_REGION: &str = "eu-west-1";

/// How many devices this suite activates Snow Device Management on. The fixture puts a
/// different number of instances on each, including none on the third.
const ACTIVATED_DEVICES: u64 = 3;

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The resource type an Amazon EC2 instance carries: "Currently, Amazon EC2 instances
/// are the only supported resource type"
/// (`smithy_query(query="doc",shape="ListDeviceResources")`), and this is the AWS-wide
/// type identifier of an EC2 instance. Written out here rather than imported, so a wrong
/// value in the implementation cannot make this test pass.
const EC2_INSTANCE_TYPE: &str = "AWS::EC2::Instance";

/// Values of `type` that name no resource type a device's resources carry, spanning the
/// shapes a client can send: another service's resource type, a sibling EC2 type, the
/// supported type in the wrong case, a bare segment of it, and the modelled
/// `@length(1..50)` floor and ceiling.
fn unmatched_types() -> Vec<String> {
    vec![
        "AWS::S3::Bucket".to_string(),
        "AWS::EC2::Volume".to_string(),
        "aws::ec2::instance".to_string(),
        "Instance".to_string(),
        "x".to_string(),
        "T".repeat(50),
    ]
}

/// Managed-device ids no account inventory holds, spanning the shapes a client can send:
/// the modelled `@length(1..64)` floor and ceiling, an id shaped like an activated one
/// but absent, and an id in another service's shape.
fn unmanaged_device_ids() -> Vec<String> {
    vec![
        "x".to_string(),
        "f".repeat(64),
        "smd-ffffffffffffffff".to_string(),
        "mdev-sim2real-ghost-70d421d7da1a".to_string(),
        managed_device_id(u64::MAX),
    ]
}

/// Pagination tokens this service never issued, each satisfying the modelled
/// `NextToken` pattern `[a-zA-Z0-9+/=]*` so the refusal comes from the handler and not
/// from the boundary validator.
fn unissued_tokens() -> Vec<String> {
    vec![
        "zz".to_string(),
        "AAA".to_string(),
        "====".to_string(),
        "ZZZZZZZZ".to_string(),
    ]
}

/// One emulator: a router carrying only this service, over one shared engine, with Snow
/// Device Management activated on [`ACTIVATED_DEVICES`] ordered devices in
/// [`SIGNED_REGION`].
///
/// The API models no device-creation operation, so the activation goes through the one
/// state seam that puts a device in the inventory.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    service_state(&*engine)
        .expect("the in-memory engine carries this service's state")
        .lock()
        .activate_devices(SIGNED_REGION, ACTIVATED_DEVICES, SystemTime::now());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The error message, or the empty string on a success.
    fn error_message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "snow-device-management.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_SCOPE);
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `ListDeviceResources`: `GET /managed-device/{managedDeviceId}/resources` with the
/// optional `type`, `maxResults` and `nextToken` query parameters.
async fn list_resources(
    router: &MultiServiceRouter,
    managed_device_id: &str,
    resource_type: Option<&str>,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Answer {
    let mut query: Vec<String> = Vec::new();
    if let Some(resource_type) = resource_type {
        query.push(format!("type={resource_type}"));
    }
    if let Some(max_results) = max_results {
        query.push(format!("maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        query.push(format!("nextToken={next_token}"));
    }
    let uri = if query.is_empty() {
        format!("/managed-device/{managed_device_id}/resources")
    } else {
        format!(
            "/managed-device/{managed_device_id}/resources?{}",
            query.join("&")
        )
    };
    call(router, "GET", &uri, None).await
}

/// `DescribeDeviceEc2Instances`, the implemented reader this suite round-trips against.
async fn describe_instances(
    router: &MultiServiceRouter,
    managed_device_id: &str,
    instance_ids: &[String],
) -> Answer {
    call(
        router,
        "POST",
        &format!("/managed-device/{managed_device_id}/resources/ec2/describe"),
        Some(json!({ "instanceIds": instance_ids })),
    )
    .await
}

/// The device ids of the account's inventory, in `ListDevices` order.
async fn inventory(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, "GET", "/managed-devices", None).await;
    assert_eq!(
        answer.status, 200,
        "case \"inventory\": expected HTTP 200 from ListDevices, got {} with {}",
        answer.status, answer.body
    );
    let devices = answer.body["devices"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case \"inventory\": expected a devices array, got {}",
                answer.body
            )
        })
        .clone();
    devices
        .iter()
        .enumerate()
        .map(|(index, device)| {
            let id = device["managedDeviceId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case \"inventory\": expected a managedDeviceId, got {device}")
                })
                .to_string();
            let expected =
                managed_device_id(u64::try_from(index).expect("a small index fits in u64"));
            assert_eq!(
                id, expected,
                "case \"inventory\": ListDevices must serve the inventory in index order \
                 so a device's resources are known, got {id:?} at index {index}"
            );
            id
        })
        .collect()
}

/// Assert a call succeeded and hand back its `resources` array.
fn resources_of(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body["resources"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a resources array, got {}",
                answer.body
            )
        })
        .clone()
}

/// The `id` of every reported resource, in answer order.
fn reported_ids(resources: &[Value], case: &str) -> Vec<String> {
    resources
        .iter()
        .map(|resource| {
            resource["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a resource id, got {resource}"))
                .to_string()
        })
        .collect()
}

/// Every rule a single reported `ResourceSummary` must satisfy, whatever device it came
/// from and whatever was asked for.
fn assert_modelled_summary(resource: &Value, case: &str) {
    let resource_type = resource["resourceType"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: resourceType is required, got {resource}"));
    assert_eq!(
        resource_type, EC2_INSTANCE_TYPE,
        "case {case:?}: an Amazon EC2 instance is the only resource type a device \
         reports, got {resource_type:?}"
    );
    let id = resource["id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a resource id, got {resource}"));
    let arn = resource["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a resource arn, got {resource}"));
    assert!(
        arn.ends_with(&format!(":instance/{id}")),
        "case {case:?}: the ARN names the resource it reports, got {arn:?} for {id:?}"
    );
    assert!(
        arn.starts_with(&format!("arn:aws:ec2:{SIGNED_REGION}:")),
        "case {case:?}: the ARN of an Amazon EC2 instance is scoped to the signed \
         Region, got {arn:?}"
    );
}

#[tokio::test]
async fn the_answer_is_the_addressed_devices_own_resources() {
    let router = emulator();
    let devices = inventory(&router).await;
    let mut non_empty_devices = 0usize;
    let mut empty_devices = 0usize;
    // Every id the whole inventory reported, so a resource shared between two devices
    // would be caught.
    let mut seen_ids: BTreeSet<String> = BTreeSet::new();

    // The table rows: every activated device, which differ in how many resources they
    // run (including one that runs none).
    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));
        let case = format!("device {device} ({index})");
        let answer = list_resources(&router, device, None, None, None).await;
        let resources = resources_of(&answer, &case);
        assert_eq!(
            reported_ids(&resources, &case),
            held,
            "case {case:?}: expected exactly the resources the device runs, in cache \
             order, got {}",
            answer.body
        );
        for resource in &resources {
            assert_modelled_summary(resource, &case);
            let id = resource["id"].as_str().expect("the summary carries an id");
            assert!(
                seen_ids.insert(id.to_string()),
                "case {case:?}: a resource of one device is not visible through \
                 another, but {id:?} was reported twice"
            );
        }

        // Round trip: the resources this operation reports are the very instances the
        // device's cache holds, as the implemented reader reports them.
        let described = describe_instances(&router, device, &held).await;
        let described_ids: Vec<String> = described.body["instances"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an instances array, got {}",
                    described.body
                )
            })
            .iter()
            .map(|summary| {
                summary["instance"]["instanceId"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected an instanceId, got {summary}")
                    })
                    .to_string()
            })
            .collect();
        assert_eq!(
            reported_ids(&resources, &case),
            described_ids,
            "case {case:?}: the listed resources and the described instances are the \
             same records, got {} and {}",
            answer.body,
            described.body
        );

        if held.is_empty() {
            empty_devices += 1;
        } else {
            non_empty_devices += 1;
        }
    }

    assert!(
        non_empty_devices >= 1 && empty_devices >= 1,
        "case \"fixture\": the inventory must span devices that run resources and a \
         device that runs none, got {non_empty_devices} with and {empty_devices} without"
    );
}

#[tokio::test]
async fn the_type_parameter_filters_by_resource_type() {
    let router = emulator();
    let devices = inventory(&router).await;

    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));

        // The table rows: the filter value × what it selects.
        let mut rows: Vec<(Option<String>, Vec<String>)> = vec![
            (None, held.clone()),
            (Some(EC2_INSTANCE_TYPE.to_string()), held.clone()),
        ];
        for unmatched in unmatched_types() {
            rows.push((Some(unmatched), Vec::new()));
        }

        for (filter, expected) in rows {
            let case = format!("device {device} ({index}), type {filter:?}");
            let answer = list_resources(&router, device, filter.as_deref(), None, None).await;
            let resources = resources_of(&answer, &case);
            assert_eq!(
                reported_ids(&resources, &case),
                expected,
                "case {case:?}: `type` selects the resources carrying that type and is \
                 never a refusal, got {}",
                answer.body
            );
            assert!(
                answer.body["nextToken"].is_null(),
                "case {case:?}: a page that holds every selected resource carries no \
                 token, got {}",
                answer.body
            );
        }
    }
}

#[tokio::test]
async fn pages_partition_one_devices_resources() {
    let router = emulator();
    let devices = inventory(&router).await;
    // Whether any row needed more than one page, so the token path is really exercised.
    let mut multi_page_rows = 0usize;

    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));
        let count = i32::try_from(held.len()).expect("the fixture count fits in i32");

        // The table rows: the page size, from the modelled `@range(1..100)` floor
        // through one past the device's resource count to the ceiling.
        let mut sizes: Vec<i32> = vec![1, 100];
        for size in 2..=count + 1 {
            sizes.push(size);
        }
        sizes.sort_unstable();
        sizes.dedup();

        for size in sizes {
            let case = format!("device {device} ({index}), maxResults {size}");
            let mut walked: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0usize;
            loop {
                let answer =
                    list_resources(&router, device, None, Some(size), token.as_deref()).await;
                let resources = resources_of(&answer, &case);
                assert!(
                    resources.len() <= usize::try_from(size).expect("a positive size fits usize"),
                    "case {case:?}: a page never exceeds maxResults, got {}",
                    answer.body
                );
                for resource in &resources {
                    assert_modelled_summary(resource, &case);
                }
                walked.extend(reported_ids(&resources, &case));
                pages += 1;
                assert!(
                    pages <= held.len() + 2,
                    "case {case:?}: paging must terminate, got {pages} pages for {} \
                     resources",
                    held.len()
                );
                match answer.body["nextToken"].as_str() {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
            }
            assert_eq!(
                walked, held,
                "case {case:?}: the pages concatenate to the whole answer, each resource \
                 exactly once and in cache order"
            );
            if pages > 1 {
                multi_page_rows += 1;
            }
        }
    }

    assert!(
        multi_page_rows >= 1,
        "case \"fixture\": at least one row must need more than one page for the token \
         to be exercised, got {multi_page_rows}"
    );
}

#[tokio::test]
async fn a_device_the_account_does_not_manage_is_not_found() {
    let router = emulator();
    let managed = inventory(&router).await;
    let mut codes: BTreeSet<String> = BTreeSet::new();

    // The table rows: the shape of the unknown id × what was asked of it. The id is
    // resolved ahead of the filter and the page, so every row is the same refusal.
    for device in unmanaged_device_ids() {
        assert!(
            !managed.contains(&device),
            "case \"fixture\": {device:?} must not be in the managed inventory"
        );
        let asks: Vec<(Option<&str>, Option<i32>)> = vec![
            (None, None),
            (Some(EC2_INSTANCE_TYPE), None),
            (Some("AWS::S3::Bucket"), None),
            (None, Some(1)),
        ];
        for (filter, size) in asks {
            let case = format!("device {device:?} with type {filter:?} and maxResults {size:?}");
            let answer = list_resources(&router, &device, filter, size, None).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException, got {}",
                answer.body
            );
            assert!(
                !answer.error_message().is_empty(),
                "case {case:?}: the refusal carries a message, got {}",
                answer.body
            );
            codes.insert(answer.error_code().to_string());
        }
    }

    assert_eq!(
        codes.len(),
        1,
        "case \"one refusal\": an id that names no device is one outcome, whatever was \
         asked of it; got {codes:?}"
    );

    // The refusals did not disturb the devices the account does manage.
    let case = "a managed device after the refusals";
    let answer = list_resources(&router, &managed[0], None, None, None).await;
    assert_eq!(
        reported_ids(&resources_of(&answer, case), case),
        device_instance_ids(0),
        "case {case:?}: a refused call must not change what a later caller sees, got {}",
        answer.body
    );
}

#[tokio::test]
async fn a_token_this_service_never_issued_is_refused() {
    let router = emulator();
    let devices = inventory(&router).await;

    // The table rows: the unissued token × the device it is sent to, including a device
    // that runs no resource at all.
    for token in unissued_tokens() {
        for (index, device) in devices.iter().enumerate() {
            let case = format!("device {device} ({index}) with nextToken {token:?}");
            let answer = list_resources(&router, device, None, None, Some(&token)).await;
            assert_eq!(
                answer.status, 400,
                "case {case:?}: a token this service never issued is a request-shape \
                 problem, got {} with {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {}",
                answer.body
            );
            assert!(
                answer.error_message().contains("nextToken"),
                "case {case:?}: the message is built from the request, so it names the \
                 member, got {:?}",
                answer.error_message()
            );
        }
    }
}

#[tokio::test]
async fn listing_resources_changes_nothing_a_later_caller_sees() {
    let router = emulator();
    let devices = inventory(&router).await;

    // The table rows: every device, listed twice, with and without a filter and a page
    // size — a report answers identically every time.
    for (index, device) in devices.iter().enumerate() {
        let held = device_instance_ids(u64::try_from(index).expect("a small index fits in u64"));
        let before = describe_instances(&router, device, &held).await;
        for (filter, size) in [
            (None, None),
            (Some(EC2_INSTANCE_TYPE), None),
            (None, Some(1)),
        ] {
            let case = format!("device {device} ({index}), type {filter:?}, maxResults {size:?}");
            let first = list_resources(&router, device, filter, size, None).await;
            let second = list_resources(&router, device, filter, size, None).await;
            assert_eq!(
                resources_of(&first, &case),
                resources_of(&second, &case),
                "case {case:?}: ListDeviceResources is a report, so two calls must \
                 answer identically"
            );
        }
        let case = format!("device {device} ({index}) after being listed");
        let after = describe_instances(&router, device, &held).await;
        assert_eq!(
            before.body, after.body,
            "case {case:?}: reading the resources must not change what a later caller \
             sees"
        );
    }
}
