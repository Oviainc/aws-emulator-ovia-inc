// SPDX-License-Identifier: CC0-1.0

//! What makes a `CreateConnection` store a connection AT ALL, where that question is about
//! how MANY connections one global network carries and what the `Tags` the request carries
//! are allowed to be — the bounds of the create, as distinct from what a stored connection
//! reports, which `crate::connection_lifecycle` covers. Driven end to end over HTTP through
//! the real router (the harness lives in `crate::global_network_lifecycle`).
//!
//! The rules under test, each asserted over a table or a sequence that spans it:
//!
//! * a connection belongs to ONE global network, and a global network carries a bounded
//!   number of them: every create up to the bound is accepted and read back through
//!   `GetConnections`, the create one past it is refused with the modelled
//!   `ServiceQuotaExceededException` (HTTP 402, `smithy_query query="operation"
//!   operation="CreateConnection"`) naming the quota and the global network that is full,
//!   the bound counts the records the global network HOLDS — so freeing one leaves room for
//!   exactly one more — and it is per global network, so a sibling still accepts a
//!   connection;
//! * the `Tags` a create carries obey the same published tag rules a later `TagResource`
//!   does, and are checked before anything is minted: a tag no resource may carry is a
//!   field failure naming the member that carried it (400 `ValidationException`), while a
//!   request past the per-resource tag quota is the modelled
//!   `ServiceQuotaExceededException` this operation does model, and every value either rule
//!   admits — the documented maximum key and value lengths, an empty value, a non-ASCII
//!   key, the full tag allowance — is stored verbatim and read back.
//!
//! Every refusal is also asserted to store NOTHING: the connections `GetConnections`
//! reports afterwards are exactly the accepted ones. Nothing reaches into `crate::state` —
//! every connection is created, listed and read back through an operation.

use serde_json::{json, Value};

use service_com_amazonaws_networkmanager_networkmanager::state::{
    LIMIT_CONNECTIONS_PER_GLOBAL_NETWORK, LIMIT_TAGS_PER_RESOURCE,
    MAX_CONNECTIONS_PER_GLOBAL_NETWORK, SERVICE_CODE,
};

use crate::connection_lifecycle::{
    create_connection, create_connection_ok, fixture, get_connections, id_of, ids_in,
    read_connection, Fixture, CONNECTION,
};
use crate::global_network_lifecycle::{error_code, Emulator};
use crate::resource_tagging::{list_tags, tag, MAX_KEY_CHARS, MAX_TAGS, MAX_VALUE_CHARS};

/// The `Reason` a `ValidationException` of this service carries when an input member is at
/// fault (`smithy_query shape=ValidationExceptionReason`).
const REASON_FIELD: &str = "FieldValidationFailed";

/// The member the tag rules blame, as `TagResource` blames it too.
const TAGS: &str = "Tags";

/// The widest page `GetConnections` admits (`MaxResults`, range 1..500: `smithy_query
/// query="operation" operation="GetConnections"`), so a listing of a full global network
/// arrives in one page.
const WIDEST_PAGE: usize = 500;

/// The `CreateConnection` body tying the two devices of `seed` together, with `tags`
/// appended when the row carries them (`Value::Null` standing for "no `Tags` member").
fn create_body(seed: &Fixture, tags: &Value) -> Value {
    let mut body = json!({
        "DeviceId": seed.device,
        "ConnectedDeviceId": seed.connected_device,
    });
    if !tags.is_null() {
        if let Some(object) = body.as_object_mut() {
            object.insert(TAGS.to_string(), tags.clone());
        }
    }
    body
}

/// Every `ConnectionId` one global network holds, in the key order `GetConnections` lists
/// in, read in one page.
async fn held_ids(emulator: &Emulator, network: &str) -> Vec<String> {
    let (status, listed) =
        get_connections(emulator, network, &format!("maxResults={WIDEST_PAGE}")).await;
    assert_eq!(
        status, 200,
        "GetConnections({network}): expected 200, got {status} with {listed}"
    );
    ids_in(&listed)
}

/// `n` distinct tags, so a row can ask for any number of them up to and past the quota.
fn distinct_tags(n: usize) -> Value {
    Value::Array(
        (0..n)
            .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
            .collect(),
    )
}

/// The tags of `list` sorted by key, so two readers of the same stored set can be compared
/// without pinning either one's order.
fn sorted_tags(list: &Value) -> Vec<Value> {
    let mut tags = list
        .as_array()
        .unwrap_or_else(|| panic!("expected a tag array in {list}"))
        .clone();
    tags.sort_by_key(|entry| entry["Key"].as_str().unwrap_or_default().to_string());
    tags
}

#[tokio::test]
async fn create_connection_fills_the_per_global_network_quota_and_refuses_the_one_past_it() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "connection-quota").await;
    let sibling = fixture(&emulator, "connection-quota-sibling").await;
    let request = create_body(&seed, &Value::Null);

    // Every create up to the bound is accepted, and each mints a record of its own.
    let mut held: Vec<String> = Vec::new();
    for index in 0..MAX_CONNECTIONS_PER_GLOBAL_NETWORK {
        let created = create_connection_ok(&emulator, &seed.network, request.clone()).await;
        let id = id_of(&created);
        assert!(
            !held.contains(&id),
            "case connection {index}: expected a freshly minted id, got the held {id:?}"
        );
        held.push(id);
    }
    held.sort();
    assert_eq!(
        held_ids(&emulator, &seed.network).await,
        held,
        "case at the bound: the global network must hold every accepted connection"
    );

    // One past it is the modelled quota refusal, naming the quota and what is full.
    let (status, refused) = create_connection(&emulator, &seed.network, request.clone()).await;
    assert_eq!(
        status, 402,
        "case one past the bound: expected 402, got {status} with {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "ServiceQuotaExceededException",
        "case one past the bound: wrong error code in {refused}"
    );
    assert_eq!(
        refused["LimitCode"],
        json!(LIMIT_CONNECTIONS_PER_GLOBAL_NETWORK),
        "case one past the bound: expected the connection quota's LimitCode, got {refused}"
    );
    assert_eq!(
        refused["ServiceCode"],
        json!(SERVICE_CODE),
        "case one past the bound: expected this service's ServiceCode, got {refused}"
    );
    assert_eq!(
        refused["ResourceType"],
        json!(CONNECTION),
        "case one past the bound: expected the connection resource type, got {refused}"
    );
    assert_eq!(
        refused["ResourceId"],
        json!(seed.network.clone()),
        "case one past the bound: expected the full global network, got {refused}"
    );
    assert_eq!(
        held_ids(&emulator, &seed.network).await,
        held,
        "case one past the bound: a refused create must store nothing"
    );

    // The bound is per global network, so a sibling of a full one still has room.
    let elsewhere = create_connection_ok(
        &emulator,
        &sibling.network,
        create_body(&sibling, &Value::Null),
    )
    .await;
    assert_eq!(
        held_ids(&emulator, &sibling.network).await,
        vec![id_of(&elsewhere)],
        "case a sibling global network: it must hold the connection it accepted"
    );

    // The bound counts what the global network HOLDS: freeing one slot leaves room for
    // exactly one more connection, and no more than one.
    let (status, deleted) = emulator
        .delete(&format!(
            "/global-networks/{}/connections/{}",
            seed.network, held[0]
        ))
        .await;
    assert_eq!(
        status, 200,
        "case freeing one slot: DeleteConnection expected 200, got {status} with {deleted}"
    );
    let replacement = id_of(&create_connection_ok(&emulator, &seed.network, request.clone()).await);
    let (status, refused_again) = create_connection(&emulator, &seed.network, request).await;
    assert_eq!(
        status, 402,
        "case refilled to the bound: expected 402, got {status} with {refused_again}"
    );

    let mut expected: Vec<String> = held[1..].to_vec();
    expected.push(replacement);
    expected.sort();
    assert_eq!(
        held_ids(&emulator, &seed.network).await,
        expected,
        "case refilled to the bound: the global network must hold the replacement, not the removed record"
    );
}

/// One row of the accepted-tags table: the `Tags` the create carries and the list both
/// readers must answer for the connection it mints.
struct AcceptedTags {
    case: &'static str,
    tags: Value,
    stored: Value,
}

/// The rows of the accepted-tags table. They differ along how many tags the request carries
/// (none at all, one, the full allowance), along the LENGTH of a key and of a value (each
/// at the documented maximum, and an empty value), along the character set (non-ASCII), and
/// along where a reserved-looking prefix sits in the key — only a key that STARTS with it
/// is reserved.
fn accepted_tags() -> Vec<AcceptedTags> {
    vec![
        AcceptedTags {
            case: "no Tags member at all, so the connection carries none",
            tags: Value::Null,
            stored: json!([]),
        },
        AcceptedTags {
            case: "an empty Tags list",
            tags: json!([]),
            stored: json!([]),
        },
        AcceptedTags {
            case: "one tag",
            tags: json!([tag("env", "test")]),
            stored: json!([tag("env", "test")]),
        },
        AcceptedTags {
            case: "a key at the documented maximum of 128 characters",
            tags: json!([tag(&"k".repeat(MAX_KEY_CHARS), "at the key bound")]),
            stored: json!([tag(&"k".repeat(MAX_KEY_CHARS), "at the key bound")]),
        },
        AcceptedTags {
            case: "a value at the documented maximum of 256 characters",
            tags: json!([tag("at-the-value-bound", &"v".repeat(MAX_VALUE_CHARS))]),
            stored: json!([tag("at-the-value-bound", &"v".repeat(MAX_VALUE_CHARS))]),
        },
        AcceptedTags {
            case: "an empty value",
            tags: json!([tag("env", "")]),
            stored: json!([tag("env", "")]),
        },
        AcceptedTags {
            case: "a non-ascii key and value",
            tags: json!([tag("zone-\u{1f310}", "erde \u{1f310}")]),
            stored: json!([tag("zone-\u{1f310}", "erde \u{1f310}")]),
        },
        AcceptedTags {
            case: "a key carrying the reserved prefix further in rather than at the start",
            tags: json!([tag("not-aws:reserved", "allowed")]),
            stored: json!([tag("not-aws:reserved", "allowed")]),
        },
        AcceptedTags {
            case: "the whole tag allowance a resource may carry",
            tags: distinct_tags(MAX_TAGS),
            stored: distinct_tags(MAX_TAGS),
        },
    ]
}

/// One row of the refused-tags table: the `Tags` the create carries, and the status and
/// error code the refusal answers.
struct RefusedTags {
    case: &'static str,
    tags: Value,
    status: u16,
    code: &'static str,
}

/// The rows of the refused-tags table. They differ along WHICH rule the request breaks (the
/// AWS-reserved key prefix in either case, an empty key, a key and a value one character
/// past their documented maximum, one tag past the per-resource allowance) and therefore
/// along the error class the refusal belongs to: a member at fault is a field failure,
/// while a tag COUNT no resource may carry is the quota refusal.
fn refused_tags() -> Vec<RefusedTags> {
    vec![
        RefusedTags {
            case: "a key using the prefix AWS reserves",
            tags: json!([tag("aws:created-by", "test")]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "that prefix in upper case, since the check is case-insensitive",
            tags: json!([tag("AWS:Created-By", "test")]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "an empty key",
            tags: json!([tag("", "test")]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "a key one character past the documented maximum",
            tags: json!([tag(&"k".repeat(MAX_KEY_CHARS + 1), "past the key bound")]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "a value one character past the documented maximum",
            tags: json!([tag(
                "past-the-value-bound",
                &"v".repeat(MAX_VALUE_CHARS + 1)
            )]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "a valid tag beside one that breaks a rule",
            tags: json!([tag("env", "test"), tag("aws:created-by", "test")]),
            status: 400,
            code: "ValidationException",
        },
        RefusedTags {
            case: "one tag more than a resource may carry",
            tags: distinct_tags(MAX_TAGS + 1),
            status: 402,
            code: "ServiceQuotaExceededException",
        },
    ]
}

#[tokio::test]
async fn create_connection_applies_the_published_tag_rules_before_storing_anything() {
    let emulator = Emulator::new();
    let seed = fixture(&emulator, "connection-tags").await;

    let mut accepted: Vec<String> = Vec::new();
    for row in accepted_tags() {
        let created =
            create_connection_ok(&emulator, &seed.network, create_body(&seed, &row.tags)).await;
        let connection_id = id_of(&created);
        assert_eq!(
            created[TAGS], row.stored,
            "case {}: expected the create to answer the tags {}, got {created}",
            row.case, row.stored
        );

        // The answer is the STORED record: the resource's own reader and the tag reader
        // both report the same stored set.
        let stored = read_connection(&emulator, &seed.network, &connection_id).await;
        assert_eq!(
            stored[TAGS], row.stored,
            "case {}: GetConnections must answer the stored tags {}, got {stored}",
            row.case, row.stored
        );
        let arn = created["ConnectionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no ConnectionArn in {created}", row.case))
            .to_string();
        let (status, tag_list) = list_tags(&emulator, &arn).await;
        assert_eq!(
            status, 200,
            "case {}: ListTagsForResource expected 200, got {status} with {tag_list}",
            row.case
        );
        assert_eq!(
            sorted_tags(&tag_list),
            sorted_tags(&row.stored),
            "case {}: ListTagsForResource must answer the stored tags, got {tag_list}",
            row.case
        );
        accepted.push(connection_id);
    }
    accepted.sort();

    for row in refused_tags() {
        let (status, refused) =
            create_connection(&emulator, &seed.network, create_body(&seed, &row.tags)).await;
        assert_eq!(
            status, row.status,
            "case {}: expected {}, got {status} with {refused}",
            row.case, row.status
        );
        assert_eq!(
            error_code(&refused),
            row.code,
            "case {}: expected {}, got {refused}",
            row.case,
            row.code
        );
        if row.status == 400 {
            assert_eq!(
                refused["Reason"],
                json!(REASON_FIELD),
                "case {}: expected a field failure, got {refused}",
                row.case
            );
            assert_eq!(
                refused["Fields"][0]["Name"],
                json!(TAGS),
                "case {}: expected the refusal to name {TAGS}, got {refused}",
                row.case
            );
        } else {
            assert_eq!(
                refused["LimitCode"],
                json!(LIMIT_TAGS_PER_RESOURCE),
                "case {}: expected the tag quota's LimitCode, got {refused}",
                row.case
            );
            assert_eq!(
                refused["ServiceCode"],
                json!(SERVICE_CODE),
                "case {}: expected this service's ServiceCode, got {refused}",
                row.case
            );
            assert_eq!(
                refused["ResourceType"],
                json!(CONNECTION),
                "case {}: expected the connection resource type, got {refused}",
                row.case
            );
        }
    }

    // Every refusal above minted nothing: the global network holds exactly the connections
    // the accepted rows created.
    assert_eq!(
        held_ids(&emulator, &seed.network).await,
        accepted,
        "case refused creates: a refused create must store nothing"
    );
}
