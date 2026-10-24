// SPDX-License-Identifier: CC0-1.0

//! `TagResource` over HTTP: how a `/tags/{resourceArn}` call is classified, and what a
//! tag write leaves behind for every later report of the resource.
//!
//! Every case goes through the router, so it exercises routing, the percent-decoded
//! `resourceArn` label, the handler, the persisted store and serialization — and every
//! expectation is read back through `DescribeDevice` / `DescribeTask` / `ListDevices`,
//! so an answer that is not computed from persisted state cannot pass.
//!
//! The rule under test: a tagging call resolves its ARN BEFORE it looks at the tags it
//! was given — a value that is not an ARN is refused as a bad request, an ARN that names
//! no device and no task of this account and region is refused as not found, and only a
//! resolved resource's tags are validated and then merged (adding a new key, replacing
//! an existing one, leaving every other key alone). Each case is a TABLE whose rows
//! differ along what the rule generalises over: the ARN's shape, which field of it is
//! wrong, the resource kind (device / task), the tag key, the tag value, the tag count.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. Deliberately NOT the emulator's
/// default, so an ARN that resolves proves the region came from the SigV4 scope.
const SIGNED_REGION: &str = "eu-west-1";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The AWS-wide tagging quotas the service enforces on a `TagMap` the model constrains
/// with nothing: 50 tags per resource, 128 characters per key, 256 per value. Spelled
/// out here rather than imported so a wrong bound in the implementation cannot make this
/// test pass.
const MAX_TAGS: usize = 50;
const MAX_TAG_KEY_LENGTH: usize = 128;
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// How many devices this suite activates Snow Device Management on, in
/// [`SIGNED_REGION`]. The API models no device-creation operation — a device becomes
/// manageable by being ORDERED — so the inventory enters state through the one
/// activation seam; without it the account's inventory is empty and the device and task
/// operations answer the modelled `ResourceNotFoundException` (see
/// `empty_inventory_refusals.rs`).
const ACTIVATED_DEVICES: u64 = 3;

/// One emulator: a router carrying only this service, over one shared engine, with Snow
/// Device Management activated on [`ACTIVATED_DEVICES`] devices in [`SIGNED_REGION`].
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

    /// The `message` an error body carries, or the empty string when it carries none.
    fn error_message(&self) -> &str {
        self.body["message"]
            .as_str()
            .or_else(|| self.body["Message"].as_str())
            .unwrap_or_default()
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

/// Percent-encode one URI path segment the way an SDK does: an ARN carries `:` and `/`,
/// and `/tags/{resourceArn}` is a single (non-greedy) label.
fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode_label(resource_arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn describe_device(router: &MultiServiceRouter, id: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/managed-device/{id}/describe"),
        None,
    )
    .await
}

async fn describe_task(router: &MultiServiceRouter, task_id: &str) -> Answer {
    call(router, "POST", &format!("/task/{task_id}"), None).await
}

async fn list_devices(router: &MultiServiceRouter) -> Answer {
    call(router, "GET", "/managed-devices", None).await
}

/// Assert a call succeeded and hand back its body.
fn ok(answer: &Answer, case: &str) -> Value {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    answer.body.clone()
}

/// Assert a call was refused with `status` and exactly the wire error code `code`.
///
/// The comparison is EXACT (after dropping any `namespace#` prefix): the whole point of
/// this family's refusals is that the published code is `NotFoundException` rather than
/// the modelled `ResourceNotFoundException`, and a substring test cannot tell those two
/// apart.
fn assert_refused(answer: &Answer, status: u16, code: &str, case: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status}, got {} with {}",
        answer.status, answer.body
    );
    let actual = answer
        .error_code()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        actual, code,
        "case {case:?}: expected the wire error code {code:?}, got {}",
        answer.body
    );
}

/// A required string member of a JSON object.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The tags a report carries, as a comparable map.
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let object = body["tags"]
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags object, got {body}"));
    object
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: tag {key:?} must be a string, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// The first seeded device, as `(managedDeviceId, managedDeviceArn)`.
async fn a_device(router: &MultiServiceRouter, case: &str) -> (String, String) {
    let body = ok(&list_devices(router).await, case);
    let devices = body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a devices array, got {body}"));
    let device = devices
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the emulator must seed a device, got {body}"));
    (
        string(device, "managedDeviceId", case),
        string(device, "managedDeviceArn", case),
    )
}

/// Create a task over one device, as `(taskId, taskArn)`.
async fn a_task(router: &MultiServiceRouter, target: &str, case: &str) -> (String, String) {
    let body = ok(
        &call(
            router,
            "POST",
            "/task",
            Some(json!({ "targets": [target], "command": { "reboot": {} } })),
        )
        .await,
        case,
    );
    (string(&body, "taskId", case), string(&body, "taskArn", case))
}

/// The tags `DescribeDevice` reports for `id`.
async fn device_tags(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let body = ok(&describe_device(router, id).await, case);
    tags_of(&body, case)
}

/// The tags `DescribeTask` reports for `task_id`.
async fn task_tags(
    router: &MultiServiceRouter,
    task_id: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let body = ok(&describe_task(router, task_id).await, case);
    tags_of(&body, case)
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

#[tokio::test]
async fn tag_resource_classifies_the_resource_arn_before_it_looks_at_the_tags() {
    let router = emulator();
    let (device_id, device_arn) = a_device(&router, "inventory").await;
    let (_, task_arn) = a_task(&router, &device_id, "task").await;

    // Every row below carries the SAME tag map, and that map is one the service refuses
    // (a reserved `aws:` key): so whatever a row is answered with is the answer to its
    // ARN, which is exactly the ordering under test.
    let refused_tags = json!({ "aws:created-by": "test", "Name": "edge" });

    // Rows differ along WHICH part of the ARN is wrong. An ARN this operation cannot
    // parse as one of this service's own — the grammar (no `arn:` prefix, too few
    // fields, an empty required field, an account that is not twelve digits), another
    // service, a resource type this service does not own, an id outside the modelled
    // bound — is a bad request, while an ARN that DOES parse and merely names nothing
    // this account holds is not found. The region and the partition are not part of
    // resolution (the inventory and the task store are per-account, as
    // `crate::state::parse_tagged_resource_arn` documents), so no row varies them.
    let bad_request: &[(&str, String)] = &[
        ("not an ARN at all", "not-an-arn".to_string()),
        ("the bare prefix", "arn".to_string()),
        (
            "five fields, no resource",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}"),
        ),
        (
            "an empty resource field",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:"),
        ),
        (
            "an empty service field",
            format!("arn:aws::{SIGNED_REGION}:{ACCOUNT}:managed-device/smd-0000000000000000"),
        ),
        (
            "an empty partition field",
            format!("arn::snow-device-management:{SIGNED_REGION}:{ACCOUNT}:task/st-1"),
        ),
        (
            "an empty region field",
            format!("arn:aws:snow-device-management::{ACCOUNT}:task/st-1"),
        ),
        (
            "an account id that is not twelve digits",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:42:managed-device/smd-0"),
        ),
        (
            "an account id that is not digits",
            format!(
                "arn:aws:snow-device-management:{SIGNED_REGION}:00000000000a:managed-device/smd-0"
            ),
        ),
        ("the prefix mis-cased", device_arn.to_uppercase()),
        (
            "another service's ARN",
            device_arn.replace("snow-device-management", "snowball"),
        ),
        (
            "an ARN of a service that carries no account",
            "arn:aws:s3:::a-bucket/an-object".to_string(),
        ),
        (
            "a resource type this service does not own",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:execution/e-1"),
        ),
        (
            "a resource field with no type",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:smd-0"),
        ),
        (
            "a resource id past the modelled 1..64 bound",
            format!(
                "arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:managed-device/{}",
                "d".repeat(65)
            ),
        ),
    ];
    let mut bad_request_messages: BTreeSet<String> = BTreeSet::new();
    for (case, arn) in bad_request {
        let answer = tag_resource(&router, arn, refused_tags.clone()).await;
        assert_refused(&answer, 400, "BadRequestException", case);
        bad_request_messages.insert(answer.error_message().to_string());
    }
    // One sentence for the whole class: the rows carry distinct ARNs, so a single
    // message across all of them is also proof that it echoes none of them.
    assert_eq!(
        bad_request_messages.len(),
        1,
        "case \"bad request\": one condition must get one sentence, got \
         {bad_request_messages:?}"
    );

    let not_found: &[(&str, String)] = &[
        (
            "a device id nothing was seeded under",
            format!(
                "arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:\
                 managed-device/smd-ffffffffffffffff"
            ),
        ),
        (
            "a task id nothing minted",
            format!("arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:task/st-ffffffff"),
        ),
        (
            "a resource id at the modelled 64-character ceiling",
            format!(
                "arn:aws:snow-device-management:{SIGNED_REGION}:{ACCOUNT}:managed-device/{}",
                "d".repeat(64)
            ),
        ),
        (
            "the device's resource type with the task's id",
            task_arn.replace("task/", "managed-device/"),
        ),
    ];
    let mut not_found_messages: BTreeSet<String> = BTreeSet::new();
    for (case, arn) in not_found {
        let answer = tag_resource(&router, arn, refused_tags.clone()).await;
        assert_refused(&answer, 404, "NotFoundException", case);
        // The refusal states the condition and nothing else: echoing the ARN back would
        // confirm what the caller asked about, including another account's resource.
        assert!(
            !answer.error_message().contains(arn.as_str()),
            "case {case:?}: the not-found message must not echo the ARN, got {}",
            answer.body
        );
        not_found_messages.insert(answer.error_message().to_string());
    }
    assert_eq!(
        not_found_messages.len(),
        1,
        "case \"not found\": one condition must get one sentence, got {not_found_messages:?}"
    );

    // The same tag map, on an ARN that DOES resolve, is answered by the tag rule — the
    // other side of the ordering: resolution first, tags second.
    for (case, arn) in [("a seeded device", &device_arn), ("a created task", &task_arn)] {
        let answer = tag_resource(&router, arn, refused_tags.clone()).await;
        assert_refused(&answer, 400, "ValidationException", case);
    }

    // And an ARN of an account this emulator does not serve is DENIED rather than
    // reported absent, which would disclose whether that account holds the resource.
    for (case, arn) in [
        ("another account's device", device_arn.replace(ACCOUNT, "123456789012")),
        ("another account's task", task_arn.replace(ACCOUNT, "210987654321")),
    ] {
        let answer = tag_resource(&router, &arn, refused_tags.clone()).await;
        assert_refused(&answer, 403, "AccessDeniedException", case);
    }

    // And none of those refusals wrote anything.
    assert_eq!(
        device_tags(&router, &device_id, "after the refusals").await,
        BTreeMap::new(),
        "case \"after the refusals\": a refused TagResource must persist nothing"
    );
}

#[tokio::test]
async fn tag_resource_enforces_the_tag_rules_on_a_resolved_resource() {
    // Rows differ along the tag key, the tag value and the tag count. A row runs against
    // its own emulator, so one row's accepted tags cannot change the next row's answer.
    struct Row {
        case: &'static str,
        tags: Value,
        accepted: bool,
    }

    let long_key: String = "k".repeat(MAX_TAG_KEY_LENGTH);
    let too_long_key: String = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    let long_value: String = "v".repeat(MAX_TAG_VALUE_LENGTH);
    let too_long_value: String = "v".repeat(MAX_TAG_VALUE_LENGTH + 1);
    let full_map: Value = (0..MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let over_full_map: Value = (0..=MAX_TAGS)
        .map(|index| (format!("key-{index:02}"), json!("v")))
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let rows = vec![
        Row {
            case: "one ordinary tag",
            tags: json!({ "Name": "edge-one" }),
            accepted: true,
        },
        Row {
            case: "an empty value",
            tags: json!({ "Name": "" }),
            accepted: true,
        },
        Row {
            case: "no tags at all",
            tags: json!({}),
            accepted: true,
        },
        Row {
            case: "a key that only looks reserved",
            tags: json!({ "awsome": "yes", "AWSNOTCOLON": "yes" }),
            accepted: true,
        },
        Row {
            case: "an empty key",
            tags: json!({ "": "v" }),
            accepted: false,
        },
        Row {
            case: "the reserved prefix",
            tags: json!({ "aws:cost-center": "42" }),
            accepted: false,
        },
        Row {
            case: "the reserved prefix, upper case",
            tags: json!({ "AWS:cost-center": "42" }),
            accepted: false,
        },
        Row {
            case: "the reserved prefix beside an acceptable tag",
            tags: json!({ "Name": "edge", "aws:cost-center": "42" }),
            accepted: false,
        },
        Row {
            case: "a key at the length ceiling",
            tags: json!({ long_key.clone(): "v" }),
            accepted: true,
        },
        Row {
            case: "a key one past the length ceiling",
            tags: json!({ too_long_key.clone(): "v" }),
            accepted: false,
        },
        Row {
            case: "a value at the length ceiling",
            tags: json!({ "Name": long_value.clone() }),
            accepted: true,
        },
        Row {
            case: "a value one past the length ceiling",
            tags: json!({ "Name": too_long_value.clone() }),
            accepted: false,
        },
        Row {
            case: "exactly as many tags as a resource may carry",
            tags: full_map,
            accepted: true,
        },
        Row {
            case: "one tag more than a resource may carry",
            tags: over_full_map,
            accepted: false,
        },
    ];

    for row in &rows {
        let router = emulator();
        let (device_id, device_arn) = a_device(&router, row.case).await;
        let answer = tag_resource(&router, &device_arn, row.tags.clone()).await;
        let expected: BTreeMap<String, String> = if row.accepted {
            ok(&answer, row.case);
            row.tags
                .as_object()
                .expect("every row's tags are a JSON object")
                .iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().expect("every row's tags are strings").to_string(),
                    )
                })
                .collect()
        } else {
            assert_refused(&answer, 400, "ValidationException", row.case);
            BTreeMap::new()
        };

        // Whatever the answer was, the stored tags are read back through DescribeDevice:
        // an accepted map is there in full, a refused one left nothing behind.
        assert_eq!(
            device_tags(&router, &device_id, row.case).await,
            expected,
            "case {:?}: DescribeDevice must report exactly the tags TagResource persisted",
            row.case
        );
    }
}

#[tokio::test]
async fn tag_resource_adds_and_replaces_tags_every_later_report_then_carries() {
    // One write per row, applied in order to the same resource: the rows differ in
    // whether they add a key, replace one, add several, or name none at all.
    struct Write {
        case: &'static str,
        tags: Value,
        /// The whole tag map the resource must report after this write.
        expected: &'static [(&'static str, &'static str)],
    }

    let writes = vec![
        Write {
            case: "the first tag",
            tags: json!({ "Name": "edge-1" }),
            expected: &[("Name", "edge-1")],
        },
        Write {
            case: "two more keys",
            tags: json!({ "Env": "prod", "Owner": "ops" }),
            expected: &[("Env", "prod"), ("Name", "edge-1"), ("Owner", "ops")],
        },
        Write {
            case: "replacing one value",
            tags: json!({ "Name": "edge-2" }),
            expected: &[("Env", "prod"), ("Name", "edge-2"), ("Owner", "ops")],
        },
        Write {
            case: "an empty map changes nothing",
            tags: json!({}),
            expected: &[("Env", "prod"), ("Name", "edge-2"), ("Owner", "ops")],
        },
        Write {
            case: "replacing one and adding one",
            tags: json!({ "Env": "staging", "Tier": "edge" }),
            expected: &[
                ("Env", "staging"),
                ("Name", "edge-2"),
                ("Owner", "ops"),
                ("Tier", "edge"),
            ],
        },
    ];

    // The same table against both resource kinds this operation addresses: "the ARN of
    // the device or task".
    let router = emulator();
    let (device_id, device_arn) = a_device(&router, "inventory").await;
    let (task_id, task_arn) = a_task(&router, &device_id, "task").await;

    for Write {
        case,
        tags,
        expected,
    } in &writes
    {
        let device_case = format!("device: {case}");
        ok(
            &tag_resource(&router, &device_arn, tags.clone()).await,
            &device_case,
        );
        assert_eq!(
            device_tags(&router, &device_id, &device_case).await,
            map(expected),
            "case {device_case:?}: DescribeDevice must report the merged tag map"
        );
        // A read is a report: asking twice must answer the same.
        assert_eq!(
            device_tags(&router, &device_id, &device_case).await,
            map(expected),
            "case {device_case:?}: a second DescribeDevice must answer identically"
        );
        // And the list projection reads the same record.
        let listed = ok(&list_devices(&router).await, &device_case);
        let device = listed["devices"]
            .as_array()
            .and_then(|devices| {
                devices
                    .iter()
                    .find(|device| device["managedDeviceId"].as_str() == Some(device_id.as_str()))
            })
            .unwrap_or_else(|| panic!("case {device_case:?}: {device_id} must be listed"))
            .clone();
        assert_eq!(
            tags_of(&device, &device_case),
            map(expected),
            "case {device_case:?}: ListDevices must project the same stored tags"
        );

        let task_case = format!("task: {case}");
        ok(
            &tag_resource(&router, &task_arn, tags.clone()).await,
            &task_case,
        );
        assert_eq!(
            task_tags(&router, &task_id, &task_case).await,
            map(expected),
            "case {task_case:?}: DescribeTask must report the merged tag map"
        );
    }
}
