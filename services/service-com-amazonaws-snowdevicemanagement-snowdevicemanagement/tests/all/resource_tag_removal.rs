// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` end to end over HTTP: which keys a removal removes, and how a
//! `resourceArn` is resolved.
//!
//! Every case goes through the router, so it exercises routing, the `@httpLabel`
//! `resourceArn`, the repeated `@httpQuery` `tagKeys`, the handler, the persisted store
//! and serialization — the same path a client takes. Nothing reaches into the state
//! struct, and every remainder is read back through an implemented reporting operation
//! (`DescribeTask` / `DescribeDevice` / `ListDevices`), so an answer that is not
//! computed from persisted state cannot pass.
//!
//! The rules under test:
//!
//! 1. `UntagResource` removes exactly the keys it names from the addressed record and
//!    leaves every other key in place; a key the record does not carry changes nothing,
//!    so the call is idempotent.
//! 2. `resourceArn` is resolved by PARSING it: a value that is not an ARN of this
//!    service naming `managed-device/<id>` or `task/<id>` is refused HTTP 400
//!    `BadRequestException`, an ARN for another account is refused 403
//!    `AccessDeniedException`, and one for this account naming no stored record is
//!    refused 404 `NotFoundException` — in each case with nothing persisted.
//!
//! Each case is a TABLE whose rows differ along what the rules generalise over — the
//! resource kind, the tag count, the key form, which keys are removed, and the ARN field
//! that is wrong — never a single recorded request.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::state::service_state;
use service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request in this suite signs for. Deliberately NOT the emulator's
/// default, so an ARN carrying it proves the region came from the SigV4 scope.
const SIGNED_REGION: &str = "ap-southeast-2";

/// The canonical account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// An account that is not the one this emulator serves.
const FOREIGN_ACCOUNT: &str = "210987654321";

/// This service's ARN namespace / sigv4 signing name, from
/// `smithy_query(query="service_info")`.
const ARN_SERVICE: &str = "snow-device-management";

/// The modelled bound on `ManagedDeviceId` and `TaskId` (`@length(min: 1, max: 64)`),
/// spelled out here rather than imported so a wrong bound in the implementation cannot
/// make this test pass.
const RESOURCE_ID_CEILING: usize = 64;

/// How this service words a `resourceArn` its tagging APIs cannot parse, as the
/// behavioural corpus recorded it. Spelled out here rather than imported, and asserted
/// on EVERY malformed row, so the wording has to state the refusal class instead of
/// being fitted to one request.
const INVALID_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// How this service words a `resourceArn` that parses but addresses no device and no
/// task, as the behavioural corpus recorded it. Asserted on every not-found row, so the
/// wording has to state the class rather than one request's resource.
const NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// A SigV4 credential scope naming this service, which is what routes the request.
const SIGNED_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/ap-southeast-2/snow-device-management/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// How many devices this suite activates Snow Device Management on, in
/// [`SIGNED_REGION`]. The API models no device-creation operation — a device becomes
/// manageable by being ORDERED — so the inventory enters state through the one
/// activation seam; without it the account's inventory is empty and the device and
/// task operations answer the modelled `ResourceNotFoundException` (see
/// `empty_inventory_refusals.rs`).
const ACTIVATED_DEVICES: u64 = 3;

/// One emulator: a router carrying only this service, over one shared engine, with
/// Snow Device Management activated on [`ACTIVATED_DEVICES`] devices in
/// [`SIGNED_REGION`].
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

    /// The error message, or the empty string when there is none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "snow-device-management.ap-southeast-2.amazonaws.com")
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

/// Percent-encode one URI component, keeping only the RFC 3986 unreserved set, so a
/// `resourceArn` label carrying `/` or `:` and a tag key carrying ` `, `+` or `=`
/// survive the round trip to the server's decoder.
fn encode(component: &str) -> String {
    let mut out = String::with_capacity(component.len());
    for byte in component.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(*byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — one `tagKeys` pair per key, which is how the
/// modelled `@httpQuery` list travels.
async fn untag_resource(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("/tags/{}?{query}", encode(resource_arn));
    call(router, "DELETE", &uri, None).await
}

async fn create_task(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/task", Some(request)).await
}

async fn describe_task(router: &MultiServiceRouter, task_id: &str) -> Answer {
    call(router, "POST", &format!("/task/{task_id}"), None).await
}

async fn describe_device(router: &MultiServiceRouter, device_id: &str) -> Answer {
    call(
        router,
        "POST",
        &format!("/managed-device/{device_id}/describe"),
        None,
    )
    .await
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

/// A required string member of a JSON object.
fn string(body: &Value, member: &str, case: &str) -> String {
    body[member]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a string {member}, got {body}"))
        .to_string()
}

/// The `tags` map of a report, as a comparable map.
fn tags_of(body: &Value, case: &str) -> BTreeMap<String, String> {
    let Some(tags) = body.get("tags") else {
        panic!("case {case:?}: expected a tags member, got {body}");
    };
    if tags.is_null() {
        return BTreeMap::new();
    }
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a map, got {body}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a string tag value, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

/// An ARN built field by field, so a row can make exactly one field wrong.
fn arn(service: &str, region: &str, account: &str, resource: &str) -> String {
    format!("arn:aws:{service}:{region}:{account}:{resource}")
}

/// Create a task carrying `tags` and hand back `(taskId, taskArn)`.
async fn task_with_tags(
    router: &MultiServiceRouter,
    device_id: &str,
    token: &str,
    tags: &[(&str, &str)],
) -> (String, String) {
    let case = format!("CreateTask({token})");
    let tag_map: serde_json::Map<String, Value> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::String((*value).to_string())))
        .collect();
    let answer = create_task(
        router,
        json!({
            "targets": [device_id],
            "command": {"reboot": {}},
            "clientToken": token,
            "tags": Value::Object(tag_map),
        }),
    )
    .await;
    let body = ok(&answer, &case);
    (
        string(&body, "taskId", &case),
        string(&body, "taskArn", &case),
    )
}

/// The first seeded device, as `ListDevices` reports it: `(managedDeviceId, arn)`.
async fn a_seeded_device(router: &MultiServiceRouter) -> (String, String) {
    let case = "ListDevices";
    let answer = list_devices(router).await;
    let body = ok(&answer, case);
    let devices = body["devices"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a devices array, got {body}"))
        .clone();
    let device = devices
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the inventory must hold a device, got {body}"));
    (
        string(device, "managedDeviceId", case),
        string(device, "managedDeviceArn", case),
    )
}

/// Assert a removal was accepted: HTTP 200 and an empty (`Unit`) payload.
fn assert_removed(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {}",
        answer.status, answer.body
    );
    let empty = answer.body.is_null()
        || answer
            .body
            .as_object()
            .is_some_and(|object| object.is_empty());
    assert!(
        empty,
        "case {case:?}: UntagResource models no output, so the body must be empty, got {}",
        answer.body
    );
}

/// Assert a refusal carried `status` and error code `code`.
fn assert_refused(answer: &Answer, status: u16, code: &str, case: &str) {
    assert_eq!(
        answer.status, status,
        "case {case:?}: expected HTTP {status} {code}, got {} with {}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().contains(code),
        "case {case:?}: expected error code {code}, got {}",
        answer.body
    );
}

/// Rule 1, on tasks: a removal removes exactly the keys it names, leaves the rest, and
/// says the same thing when it is repeated.
///
/// The rows differ along the tag count, the key and value forms (the characters AWS tag
/// keys admit — letters, digits, space and `+ - = . _ : / @`; `TagMap` itself is
/// modelled `key: String, value: String` with no constraint, per
/// `smithy_query(query="shape",shape="TagMap")`), and which keys the request names:
/// one, several, all, a repeated key, and a key that was never there. `tagKeys` is
/// modelled `@required`, so a request naming no key at all is not a row here: the
/// router rejects it before the handler, as it must.
#[tokio::test]
async fn untag_resource_removes_only_the_named_keys_and_is_idempotent() {
    struct Row {
        label: &'static str,
        tags: &'static [(&'static str, &'static str)],
        remove: &'static [&'static str],
        left: &'static [(&'static str, &'static str)],
    }

    let rows = &[
        Row {
            label: "one of one",
            tags: &[("owner", "fleet-ops")],
            remove: &["owner"],
            left: &[],
        },
        Row {
            label: "one of three",
            tags: &[("owner", "fleet-ops"), ("env", "prod"), ("tier", "edge")],
            remove: &["env"],
            left: &[("owner", "fleet-ops"), ("tier", "edge")],
        },
        Row {
            label: "two of three",
            tags: &[("owner", "fleet-ops"), ("env", "prod"), ("tier", "edge")],
            remove: &["owner", "tier"],
            left: &[("env", "prod")],
        },
        Row {
            label: "every key",
            tags: &[("owner", "fleet-ops"), ("env", "prod")],
            remove: &["owner", "env"],
            left: &[],
        },
        Row {
            label: "a key that was never there",
            tags: &[("owner", "fleet-ops")],
            remove: &["absent"],
            left: &[("owner", "fleet-ops")],
        },
        Row {
            label: "the same key twice",
            tags: &[("owner", "fleet-ops"), ("env", "prod")],
            remove: &["owner", "owner"],
            left: &[("env", "prod")],
        },
        Row {
            label: "keys carrying the punctuation AWS tag keys admit",
            tags: &[
                ("cost:center", "cc-1"),
                ("path/segment", "a/b"),
                ("plus+equals=", "v+w=x"),
                ("dot.dash-under_", "y"),
                ("with space", "z"),
                ("at@sign", "w"),
            ],
            remove: &["path/segment", "plus+equals=", "with space"],
            left: &[
                ("cost:center", "cc-1"),
                ("dot.dash-under_", "y"),
                ("at@sign", "w"),
            ],
        },
        Row {
            label: "a unicode key and value",
            tags: &[("équipe", "préproduction"), ("owner", "fleet-ops")],
            remove: &["équipe"],
            left: &[("owner", "fleet-ops")],
        },
    ];

    let router = emulator();
    let (device_id, _) = a_seeded_device(&router).await;

    for (index, row) in rows.iter().enumerate() {
        let label = row.label;
        let expected: BTreeMap<String, String> = row
            .left
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let (task_id, task_arn) =
            task_with_tags(&router, &device_id, &format!("token-{index}"), row.tags).await;

        let answer = untag_resource(&router, &task_arn, row.remove).await;
        assert_removed(&answer, label);

        let described = ok(&describe_task(&router, &task_id).await, label);
        assert_eq!(
            tags_of(&described, label),
            expected,
            "case {label:?}: removing {:?} from {:?} must leave exactly {expected:?}, got {}",
            row.remove,
            row.tags,
            described
        );

        // Idempotence: the same request again removes nothing more and still succeeds.
        let again = untag_resource(&router, &task_arn, row.remove).await;
        assert_removed(&again, label);
        let described_again = ok(&describe_task(&router, &task_id).await, label);
        assert_eq!(
            tags_of(&described_again, label),
            expected,
            "case {label:?}: a repeated removal must change nothing, got {described_again}"
        );
    }
}

/// Rule 1, on devices, and the isolation between records: a removal touches only the
/// record its ARN names.
#[tokio::test]
async fn untag_resource_touches_only_the_record_its_arn_names() {
    let router = emulator();
    let (device_id, device_arn) = a_seeded_device(&router).await;

    let (kept_id, _) = task_with_tags(
        &router,
        &device_id,
        "token-kept",
        &[("owner", "fleet-ops"), ("env", "prod")],
    )
    .await;
    let (stripped_id, stripped_arn) = task_with_tags(
        &router,
        &device_id,
        "token-stripped",
        &[("owner", "fleet-ops"), ("env", "prod")],
    )
    .await;

    let case = "removing owner from one of two identically tagged tasks";
    assert_removed(&untag_resource(&router, &stripped_arn, &["owner"]).await, case);

    let stripped = ok(&describe_task(&router, &stripped_id).await, case);
    assert_eq!(
        tags_of(&stripped, case),
        BTreeMap::from([("env".to_string(), "prod".to_string())]),
        "case {case:?}: the addressed task must have lost only owner, got {stripped}"
    );
    let kept = ok(&describe_task(&router, &kept_id).await, case);
    assert_eq!(
        tags_of(&kept, case),
        BTreeMap::from([
            ("env".to_string(), "prod".to_string()),
            ("owner".to_string(), "fleet-ops".to_string()),
        ]),
        "case {case:?}: the other task must be untouched, got {kept}"
    );

    // A device ARN addresses the inventory record, which this fixture seeds untagged:
    // removing a key from it is accepted and leaves it untagged.
    let case = "a device ARN the emulator itself issued";
    assert_removed(
        &untag_resource(&router, &device_arn, &["owner", "env"]).await,
        case,
    );
    let described = ok(&describe_device(&router, &device_id).await, case);
    assert_eq!(
        tags_of(&described, case),
        BTreeMap::new(),
        "case {case:?}: the device must still report no tags, got {described}"
    );

    // The ARN's region is not part of the key: the inventory and the task store are
    // per-account, so a device ARN naming another region addresses the same record.
    let case = "a device ARN naming another region of this account";
    let other_region = arn(
        ARN_SERVICE,
        "eu-west-1",
        ACCOUNT,
        &format!("managed-device/{device_id}"),
    );
    assert_removed(&untag_resource(&router, &other_region, &["owner"]).await, case);
}

/// Rule 2, the malformed half: a `resourceArn` that is not an ARN of this service
/// naming a device or a task is refused HTTP 400 `BadRequestException`, and nothing is
/// removed.
///
/// The rows differ along WHICH field is wrong — the `arn` prefix, the field count, the
/// service, the region, the account, the resource type, the id — because the rule is
/// about parsing the ARN, not about one recorded string. The id-length rows sit on the
/// modelled `@length(min: 1, max: 64)` bound of `ManagedDeviceId` / `TaskId`: 64 is
/// inside it (so it parses, and is answered by the not-found case below) and 65 is not.
#[tokio::test]
async fn an_unparseable_resource_arn_is_refused_as_a_bad_request() {
    let router = emulator();
    let (device_id, _) = a_seeded_device(&router).await;
    let (task_id, task_arn) = task_with_tags(
        &router,
        &device_id,
        "token-malformed",
        &[("owner", "fleet-ops"), ("env", "prod")],
    )
    .await;

    let too_long_id = "s".repeat(RESOURCE_ID_CEILING + 1);
    let cases: Vec<(String, String)> = vec![
        ("not an ARN at all".to_string(), "this-is-not-an-arn".to_string()),
        (
            "the resource id alone".to_string(),
            task_id.clone(),
        ),
        (
            "the arn prefix miscased".to_string(),
            format!("ARN:aws:{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}:task/{task_id}"),
        ),
        (
            "an empty partition".to_string(),
            format!("arn::{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}:task/{task_id}"),
        ),
        (
            "too few fields".to_string(),
            format!("arn:aws:{ARN_SERVICE}:{SIGNED_REGION}:{ACCOUNT}"),
        ),
        (
            "another service".to_string(),
            arn("snowball", SIGNED_REGION, ACCOUNT, &format!("task/{task_id}")),
        ),
        (
            "an empty region".to_string(),
            arn(ARN_SERVICE, "", ACCOUNT, &format!("task/{task_id}")),
        ),
        (
            "an account that is not 12 digits".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, "12345", &format!("task/{task_id}")),
        ),
        (
            "an account that is not digits".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                "00000000000a",
                &format!("task/{task_id}"),
            ),
        ),
        (
            "a resource type this service does not have".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                ACCOUNT,
                &format!("device/{task_id}"),
            ),
        ),
        (
            "a resource with no id".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, "task/"),
        ),
        (
            "a resource with no type separator".to_string(),
            arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, "task"),
        ),
        (
            "an id one past the modelled length ceiling".to_string(),
            arn(
                ARN_SERVICE,
                SIGNED_REGION,
                ACCOUNT,
                &format!("managed-device/{too_long_id}"),
            ),
        ),
    ];

    let intact = BTreeMap::from([
        ("env".to_string(), "prod".to_string()),
        ("owner".to_string(), "fleet-ops".to_string()),
    ]);
    for (label, resource_arn) in &cases {
        let answer = untag_resource(&router, resource_arn, &["owner"]).await;
        assert_refused(&answer, 400, "BadRequestException", label);
        assert_eq!(
            answer.message(),
            INVALID_ARN_MESSAGE,
            "case {label:?}: every unparseable resourceArn must be refused with the one \
             class message, got {}",
            answer.body
        );

        let described = ok(&describe_task(&router, &task_id).await, label);
        assert_eq!(
            tags_of(&described, label),
            intact,
            "case {label:?}: a refused removal must persist nothing, got {described}"
        );
    }

    // The positive control for the same table: the well-formed ARN of the same task is
    // served, so the refusals above are about the ARN and not about the operation.
    let case = "the task's own ARN";
    assert_removed(&untag_resource(&router, &task_arn, &["owner"]).await, case);
}

/// Rule 2, the resolvable halves: a well-formed ARN of this account naming no stored
/// record is 404 `NotFoundException`, and one naming another account is 403
/// `AccessDeniedException` whatever this account holds.
#[tokio::test]
async fn a_well_formed_arn_for_no_such_resource_or_another_account_is_refused() {
    let router = emulator();
    let (device_id, _) = a_seeded_device(&router).await;
    let (task_id, _) = task_with_tags(
        &router,
        &device_id,
        "token-resolution",
        &[("owner", "fleet-ops")],
    )
    .await;

    // An id inside the modelled length bound that names no record: one per resource
    // kind, plus the 64-character ceiling itself.
    let longest_id = "s".repeat(RESOURCE_ID_CEILING);
    let absent = vec![
        (
            "a device id that names no device",
            "managed-device/smd-ffffffffffffffff".to_string(),
        ),
        ("a task id that names no task", "task/st-ffffffffffffffff".to_string()),
        (
            "a task id at the modelled length ceiling",
            format!("task/{longest_id}"),
        ),
        (
            "a device id that is in fact a task id",
            format!("managed-device/{task_id}"),
        ),
        (
            "a task id that is in fact a device id",
            format!("task/{device_id}"),
        ),
    ];
    for (label, resource) in &absent {
        let answer =
            untag_resource(&router, &arn(ARN_SERVICE, SIGNED_REGION, ACCOUNT, resource), &["owner"])
                .await;
        assert_refused(&answer, 404, "NotFoundException", label);
        assert_eq!(
            answer.message(),
            NOT_FOUND_MESSAGE,
            "case {label:?}: every resolvable ARN naming no record must be refused with \
             the one class message, got {}",
            answer.body
        );
    }

    // Another account's ARN: refused as denied, for a resource this account HAS (so the
    // answer cannot be read as "it is not there") and for one it has not alike.
    let foreign = vec![
        (
            "another account's ARN for a device this account has",
            format!("managed-device/{device_id}"),
        ),
        (
            "another account's ARN for a task this account has",
            format!("task/{task_id}"),
        ),
        (
            "another account's ARN for a resource nobody has",
            "task/st-ffffffffffffffff".to_string(),
        ),
    ];
    for (label, resource) in &foreign {
        let answer = untag_resource(
            &router,
            &arn(ARN_SERVICE, SIGNED_REGION, FOREIGN_ACCOUNT, resource),
            &["owner"],
        )
        .await;
        assert_refused(&answer, 403, "AccessDeniedException", label);
    }

    // None of the refusals above removed anything.
    let case = "after every refusal";
    let described = ok(&describe_task(&router, &task_id).await, case);
    assert_eq!(
        tags_of(&described, case),
        BTreeMap::from([("owner".to_string(), "fleet-ops".to_string())]),
        "case {case:?}: the task must still carry its tag, got {described}"
    );
}
