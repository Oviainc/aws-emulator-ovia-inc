// SPDX-License-Identifier: CC0-1.0

//! The destination lifecycle, end to end over the wire: `CreateDestination` ->
//! `GetDestination` -> `UpdateDestination` -> `DeleteDestination`.
//!
//! RULE: a destination is identified by its `Name` inside the region its create was
//! signed for, and its record is the one thing every operation of the surface reads and
//! writes. `UpdateDestination` (PUT `/destinations/{Name}`, success 201) REPLACES each
//! member its request names, leaves every member it does not name as it was, and moves the
//! record's `UpdatedAt` while its `CreatedAt` stays where the create put it, so
//! `GetDestination` reports exactly the merged record. `DeleteDestination` (DELETE
//! `/destinations/{Name}`, success 200) removes it, after which the name resolves to
//! nothing on an independent read and a mutation of it fails the same way — every
//! operation of the surface states an absent name with the BARE modelled
//! `ResourceNotFoundException`, carrying none of its optional members. A create for a
//! name the region already holds a destination under is the modelled `ConflictException`
//! (HTTP 409) and leaves the stored destination untouched, while a distinct name still
//! succeeds. A destination delivers to a target of the SAME region as the call, so a
//! `DeliveryDestinationArn` naming a target of another region is the modelled
//! `ValidationException` (HTTP 400) — on a create and on an update alike — and changes
//! nothing. Every one of these is regional: only the region whose endpoint created the
//! destination reads, updates or deletes it.
//!
//! The recorded units this was derived from
//! (`iot-managed-integrations.destination-lifecycle-and-update` and
//! `…-destination-duplicate-name-conflict-rejected`) are ONE row of the tables below. The
//! rows differ along what the rule generalises over: which members an update names, the
//! form and length of `Name` and `Description` inside their modelled
//! `@pattern`/`@length`, which member of a stored destination the second create differs
//! in, the operation a dead name is addressed with, and the region the request is signed
//! for.
//!
//! Every case drives the real restJson1 request path (`MultiServiceRouter::dispatch`), so
//! the routes, statuses, bodies and modelled errors asserted here are the ones a client
//! sees.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::ops::types::DELIVERY_DESTINATION_TYPE_VALUES;
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::state::{
    already_exists_message, DELIVERY_DESTINATION_REGION_MISMATCH_MESSAGE,
    DESTINATION_RESOURCE_SUBJECT,
};
use service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's requests are signed for unless a case names another.
const REGION: &str = "us-east-1";

/// The wire error codes the model's `@httpError` shapes are rendered under.
const VALIDATION: &str = "ValidationException";
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The `@length` maximum of `DestinationDescription`, in characters
/// (`smithy_query(query="operation",operation="UpdateDestination")` -> `Description` len
/// 1..256, pattern `^[0-9A-Za-z_\- ]+$`).
const DESCRIPTION_MAX_LENGTH: usize = 256;

/// One HTTP answer, reduced to what the assertions read.
struct Reply {
    status: u16,
    /// The restJson1 error code, from `x-amzn-errortype`.
    error_type: Option<String>,
    body: Value,
}

impl Reply {
    fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    fn string(&self, name: &str) -> Option<&str> {
        self.member(name).and_then(Value::as_str)
    }

    fn seconds(&self, name: &str) -> Option<f64> {
        self.member(name).and_then(Value::as_f64)
    }

    /// The reason an error body carries, on the member the model declares for it.
    fn message(&self) -> &str {
        self.string("Message")
            .or_else(|| self.string("message"))
            .unwrap_or_default()
    }
}

/// A router holding one `iotmanagedintegrations` service over a fresh shared engine: the
/// same assembly `core/router` performs, with one service mounted.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here, so requests are unauthenticated.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(engine::InMemoryEngine::new())));
    router
}

/// Send one request, signed for `region`, and reduce the answer. The `Authorization`
/// credential scope is what routes the request to this service and names its region.
async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = body.map_or_else(bytes::Bytes::new, |value| {
        bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request body"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://iotmanagedintegrations.{region}.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTESTTEST/20260101/{region}/\
                 iotmanagedintegrations/aws4_request, SignedHeaders=host, Signature=0"
            ),
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        error_type,
        body,
    }
}

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`), which
/// is the account whose resources these ARNs name.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A `DeliveryDestinationArn` of `region`, inside the modelled pattern
/// (`^arn:aws:[0-9a-zA-Z]+:[0-9a-zA-Z-]+:[0-9]+:[0-9a-zA-Z]+/[0-9a-zA-Z._-]+$`).
fn stream_arn(region: &str, stream: &str) -> String {
    format!("arn:aws:kinesis:{region}:{ACCOUNT}:stream/{stream}")
}

/// A `DeliveryDestinationRoleArn` the delivery is performed under: an IAM role of THIS
/// account, which is what a destination call resolves its `RoleArn` as (a value naming no
/// role of this account is refused — `tests/all/referenced_resource_validation.rs`).
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role}")
}

/// The create body of a destination called `name` delivering into `region`, carrying the
/// four required members plus an initial description.
fn create_body(region: &str, name: &str) -> Value {
    json!({
        "Name": name,
        "DeliveryDestinationArn": stream_arn(region, "sim2real-stream-a"),
        "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
        "RoleArn": role_arn("managed-integrations"),
        "Description": "initial description",
    })
}

async fn create_destination_in(router: &MultiServiceRouter, region: &str, body: Value) -> Reply {
    call_in(router, region, "POST", "/destinations", Some(body)).await
}

/// `name` as an SDK puts it into the URI: every character outside the unreserved set
/// (RFC 3986 `ALPHA / DIGIT / "-" / "." / "_" / "~"`) percent-encoded, which is what a
/// `@httpLabel` carrying a space or a non-ASCII letter looks like on the wire.
fn encode_label(name: &str) -> String {
    let mut encoded = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

async fn get_destination_in(router: &MultiServiceRouter, region: &str, name: &str) -> Reply {
    call_in(
        router,
        region,
        "GET",
        &format!("/destinations/{}", encode_label(name)),
        None,
    )
    .await
}

async fn update_destination_in(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    body: Value,
) -> Reply {
    call_in(
        router,
        region,
        "PUT",
        &format!("/destinations/{}", encode_label(name)),
        Some(body),
    )
    .await
}

async fn delete_destination_in(router: &MultiServiceRouter, region: &str, name: &str) -> Reply {
    call_in(
        router,
        region,
        "DELETE",
        &format!("/destinations/{}", encode_label(name)),
        None,
    )
    .await
}

/// The members the modelled `ResourceNotFoundException` declares, all optional
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` -> `Message`,
/// `ResourceId`, `ResourceType`, `httpError` 404) — none of which a destination's 404
/// carries.
const NOT_FOUND_MEMBERS: [&str; 3] = ["Message", "ResourceId", "ResourceType"];

/// Assert `reply` is the modelled refusal `code` with `status`. A `ResourceNotFoundException`
/// of this surface is the BARE exception — the status and the error code alone, with none of
/// [`NOT_FOUND_MEMBERS`] on the body a client reads — so every 404 asserted here is checked
/// for that too.
fn assert_refused(case: &str, reply: &Reply, status: u16, code: &str) {
    assert_eq!(
        reply.status, status,
        "case {case:?}: expected HTTP {status}, got {} with body {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error_type.as_deref(),
        Some(code),
        "case {case:?}: expected the modelled {code}, got {:?} with body {:?}",
        reply.error_type,
        reply.body
    );
    if code == NOT_FOUND {
        for member in NOT_FOUND_MEMBERS {
            assert!(
                reply.member(member).is_none(),
                "case {case:?}: a destination's 404 carries no {member}, got {:?} in body {:?}",
                reply.member(member),
                reply.body
            );
        }
    }
}

/// Create the destination `name` in `region`, asserting the modelled 201.
async fn seed(router: &MultiServiceRouter, case: &str, region: &str, name: &str) {
    let created = create_destination_in(router, region, create_body(region, name)).await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected HTTP 201 from CreateDestination, got {} with body {:?}",
        created.status, created.body
    );
}

/// One row of the update table: which members the update names, and what the record must
/// read as afterwards.
struct UpdateCase {
    label: &'static str,
    /// The members the update sends, beside the `Name` the URI carries.
    update: Value,
}

/// The table the update rule is checked over: the rows differ along which of the four
/// updatable members the request names — one, two, all of them, none at all — and the form
/// and length of the `Description` it carries.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            label: "description-only",
            update: json!({ "Description": "updated description" }),
        },
        UpdateCase {
            label: "description-at-the-modelled-maximum",
            update: json!({
                "Description": std::iter::repeat_n('d', DESCRIPTION_MAX_LENGTH)
                    .collect::<String>(),
            }),
        },
        UpdateCase {
            label: "delivery-destination-arn-only",
            update: json!({ "DeliveryDestinationArn": stream_arn(REGION, "sim2real-stream-b") }),
        },
        UpdateCase {
            label: "role-and-type",
            update: json!({
                "RoleArn": role_arn("managed-integrations-2"),
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
            }),
        },
        UpdateCase {
            label: "every-updatable-member",
            update: json!({
                "DeliveryDestinationArn": stream_arn(REGION, "sim2real-stream-c"),
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("managed-integrations-3"),
                "Description": "every member replaced",
            }),
        },
        UpdateCase {
            label: "nothing-but-the-name",
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_leaves_the_rest() {
    for case in update_cases() {
        let label = case.label;
        let router = emulator();
        let name = "sim2real.destination";
        seed(&router, label, REGION, name).await;

        let before = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            before.status, 200,
            "case {label:?}: expected the seeded destination to be readable, got {} with \
             body {:?}",
            before.status, before.body
        );

        let updated = update_destination_in(&router, REGION, name, case.update.clone()).await;
        assert_eq!(
            updated.status, 201,
            "case {label:?}: expected the modelled 201 from UpdateDestination, got {} with \
             body {:?}",
            updated.status, updated.body
        );

        let after = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            after.status, 200,
            "case {label:?}: expected the updated destination to be readable, got {} with \
             body {:?}",
            after.status, after.body
        );
        assert_eq!(
            after.string("Name"),
            Some(name),
            "case {label:?}: an update never moves the name that identifies the record, got \
             {:?}",
            after.string("Name")
        );

        for field in [
            "DeliveryDestinationArn",
            "DeliveryDestinationType",
            "RoleArn",
            "Description",
        ] {
            let expected = case
                .update
                .get(field)
                .and_then(Value::as_str)
                .or_else(|| before.string(field));
            assert_eq!(
                after.string(field),
                expected,
                "case {label:?}: {field} must be what the update named, or what the create \
                 stored where it named none, got {:?}",
                after.string(field)
            );
        }

        assert_eq!(
            after.seconds("CreatedAt"),
            before.seconds("CreatedAt"),
            "case {label:?}: an update does not move CreatedAt, got {:?}",
            after.seconds("CreatedAt")
        );
        let updated_at = after
            .seconds("UpdatedAt")
            .unwrap_or_else(|| panic!("case {label:?}: expected an UpdatedAt timestamp"));
        let created_at = after.seconds("CreatedAt").unwrap_or_default();
        assert!(
            updated_at >= created_at,
            "case {label:?}: UpdatedAt must be the instant the update ran, got \
             {updated_at} against a CreatedAt of {created_at}"
        );
    }
}

#[tokio::test]
async fn a_destination_name_round_trips_whatever_characters_it_carries() {
    // The rows differ along the characters the `DestinationName` `@pattern`
    // (`^[\p{L}\p{N} ._-]+$`) admits: letters and digits, a space, the punctuation, a
    // non-ASCII letter, and one character.
    for name in [
        "plain",
        "with space",
        "dot.and-dash_1",
        "caffè",
        "z",
        "a b.c-d_e 1",
    ] {
        let router = emulator();
        seed(&router, name, REGION, name).await;

        let read = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            read.status, 200,
            "case {name:?}: a name the create stored must resolve on a read, got {} with \
             body {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("Name"),
            Some(name),
            "case {name:?}: the read reports the name the create stored, got {:?}",
            read.string("Name")
        );

        let updated = update_destination_in(
            &router,
            REGION,
            name,
            json!({ "Description": "updated through the same name" }),
        )
        .await;
        assert_eq!(
            updated.status, 201,
            "case {name:?}: an update resolves the same name a read does, got {} with body \
             {:?}",
            updated.status, updated.body
        );
        let after = get_destination_in(&router, REGION, name).await;
        assert_eq!(
            after.string("Description"),
            Some("updated through the same name"),
            "case {name:?}: the update reached the stored record, got {:?}",
            after.body
        );

        let deleted = delete_destination_in(&router, REGION, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {name:?}: a delete resolves the same name a read does, got {} with body \
             {:?}",
            deleted.status, deleted.body
        );
    }
}

#[tokio::test]
async fn a_deleted_destination_is_gone_and_its_name_is_unusable() {
    let router = emulator();
    let name = "sim2real.destination.doomed";
    let survivor = "sim2real.destination.kept";
    seed(&router, "delete", REGION, name).await;
    seed(&router, "delete", REGION, survivor).await;

    let deleted = delete_destination_in(&router, REGION, name).await;
    assert_eq!(
        deleted.status, 200,
        "case \"delete\": expected the modelled 200 from DeleteDestination, got {} with body \
         {:?}",
        deleted.status, deleted.body
    );

    // Gone on an independent read, which states the absence with the bare exception.
    let read = get_destination_in(&router, REGION, name).await;
    assert_refused("delete/read", &read, 404, NOT_FOUND);

    // And unusable: every mutation of the dead name fails the same way. The rows differ
    // along the operation the dead name is addressed with.
    let update = update_destination_in(
        &router,
        REGION,
        name,
        json!({ "Description": "resurrect me" }),
    )
    .await;
    assert_refused("delete/update", &update, 404, NOT_FOUND);
    let again = delete_destination_in(&router, REGION, name).await;
    assert_refused("delete/delete", &again, 404, NOT_FOUND);

    // Deleting one destination leaves every other one alone, and the freed name can be
    // created again.
    let kept = get_destination_in(&router, REGION, survivor).await;
    assert_eq!(
        kept.status, 200,
        "case \"delete/survivor\": a delete touches only the name it names, got {} with body \
         {:?}",
        kept.status, kept.body
    );
    let recreated = create_destination_in(&router, REGION, create_body(REGION, name)).await;
    assert_eq!(
        recreated.status, 201,
        "case \"delete/recreate\": the deleted name is free again, got {} with body {:?}",
        recreated.status, recreated.body
    );
}

#[tokio::test]
async fn a_duplicate_name_is_refused_and_a_distinct_name_still_succeeds() {
    let router = emulator();
    let name = "sim2real.destination.unique";
    seed(&router, "conflict", REGION, name).await;
    let stored = get_destination_in(&router, REGION, name).await;

    // The rows differ along which member of the stored destination the second create
    // differs in: the stream it delivers to, the role, the description, or nothing at all.
    let cases: Vec<(&str, Value)> = vec![
        ("same-members", create_body(REGION, name)),
        (
            "another-stream",
            json!({
                "Name": name,
                "DeliveryDestinationArn": stream_arn(REGION, "sim2real-stream-other"),
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("managed-integrations"),
            }),
        ),
        (
            "another-role-and-description",
            json!({
                "Name": name,
                "DeliveryDestinationArn": stream_arn(REGION, "sim2real-stream-a"),
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("someone-else"),
                "Description": "a different description",
            }),
        ),
    ];

    for (label, body) in cases {
        let conflict = create_destination_in(&router, REGION, body).await;
        assert_refused(label, &conflict, 409, CONFLICT);
        assert_eq!(
            conflict.message(),
            already_exists_message(DESTINATION_RESOURCE_SUBJECT),
            "case {label:?}: expected the create path's conflict reason, got {:?}",
            conflict.body
        );

        let reread = get_destination_in(&router, REGION, name).await;
        for field in [
            "DeliveryDestinationArn",
            "DeliveryDestinationType",
            "RoleArn",
            "Description",
        ] {
            assert_eq!(
                reread.string(field),
                stored.string(field),
                "case {label:?}: the refused create left {field} exactly as it was, got {:?}",
                reread.string(field)
            );
        }
        assert_eq!(
            reread.seconds("UpdatedAt"),
            stored.seconds("UpdatedAt"),
            "case {label:?}: the refused create changed nothing, so UpdatedAt stands still"
        );
    }

    // A distinct name is a distinct destination, so it still succeeds.
    for distinct in ["sim2real.destination.unique.2", "z", "another name"] {
        let created =
            create_destination_in(&router, REGION, create_body(REGION, distinct)).await;
        assert_eq!(
            created.status, 201,
            "case {distinct:?}: a distinct name is a distinct destination, got {} with body \
             {:?}",
            created.status, created.body
        );
    }
}

#[tokio::test]
async fn a_delivery_target_of_another_region_is_refused() {
    let name = "sim2real.destination.regional";

    // The rows differ along the region the call is signed for and the region its delivery
    // target lives in.
    let cases = [
        ("us-east-1", "us-west-2"),
        ("eu-central-1", "us-east-1"),
        ("us-west-2", "eu-west-1"),
    ];

    for (call_region, target_region) in cases {
        let label = format!("{call_region}-calling-{target_region}");
        let router = emulator();

        let refused = create_destination_in(
            &router,
            call_region,
            json!({
                "Name": name,
                "DeliveryDestinationArn": stream_arn(target_region, "sim2real-stream-a"),
                "DeliveryDestinationType": DELIVERY_DESTINATION_TYPE_VALUES[0],
                "RoleArn": role_arn("managed-integrations"),
            }),
        )
        .await;
        assert_refused(&label, &refused, 400, VALIDATION);
        assert_eq!(
            refused.message(),
            DELIVERY_DESTINATION_REGION_MISMATCH_MESSAGE,
            "case {label:?}: expected the region rule's reason, got {:?}",
            refused.body
        );

        // Nothing was stored.
        let read = get_destination_in(&router, call_region, name).await;
        assert_refused(&format!("{label}/read"), &read, 404, NOT_FOUND);

        // The same request with a target of the call's own region is accepted, and an
        // update that would move the target out of the region is refused, leaving the
        // stored ARN alone.
        seed(&router, &label, call_region, name).await;
        let stored = get_destination_in(&router, call_region, name).await;
        let moved = update_destination_in(
            &router,
            call_region,
            name,
            json!({ "DeliveryDestinationArn": stream_arn(target_region, "sim2real-stream-b") }),
        )
        .await;
        assert_refused(&format!("{label}/update"), &moved, 400, VALIDATION);
        let reread = get_destination_in(&router, call_region, name).await;
        assert_eq!(
            reread.string("DeliveryDestinationArn"),
            stored.string("DeliveryDestinationArn"),
            "case {label:?}: the refused update left the stored ARN as it was, got {:?}",
            reread.string("DeliveryDestinationArn")
        );
    }
}

#[tokio::test]
async fn only_the_region_that_created_a_destination_can_change_it() {
    let router = emulator();
    let name = "sim2real.destination.scoped";
    let owner = "us-east-1";
    seed(&router, "regional", owner, name).await;
    let stored = get_destination_in(&router, owner, name).await;

    // The rows differ along the region the request is signed for; none of them owns the
    // destination.
    for other in ["us-west-2", "eu-central-1", "ap-southeast-2"] {
        let read = get_destination_in(&router, other, name).await;
        assert_refused(&format!("{other}/read"), &read, 404, NOT_FOUND);

        let update = update_destination_in(
            &router,
            other,
            name,
            json!({ "Description": "not yours to change" }),
        )
        .await;
        assert_refused(&format!("{other}/update"), &update, 404, NOT_FOUND);

        let deleted = delete_destination_in(&router, other, name).await;
        assert_refused(&format!("{other}/delete"), &deleted, 404, NOT_FOUND);
    }

    // The owning region still reports exactly what it stored.
    let reread = get_destination_in(&router, owner, name).await;
    assert_eq!(
        reread.status, 200,
        "case \"regional/owner\": expected the owning region to still resolve it, got {} \
         with body {:?}",
        reread.status, reread.body
    );
    assert_eq!(
        reread.string("Description"),
        stored.string("Description"),
        "case \"regional/owner\": another region's calls changed nothing, got {:?}",
        reread.string("Description")
    );
}
