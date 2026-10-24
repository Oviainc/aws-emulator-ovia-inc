// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutAppsList`: the create-or-update writer of a
//! Firewall Manager applications list, read back through `GetAppsList` and
//! removed through `DeleteAppsList`.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rules under test:
//!
//! 1. A `PutAppsList` with no `ListId` CREATES a list: the service mints the
//!    `ListId`, the first `ListUpdateToken` and the ARN — whose Region segment
//!    is the Region the request was signed for — records the create and
//!    last-update instants and stores the caller's document unchanged, so a
//!    later read reports exactly what was written.
//! 2. A `PutAppsList` carrying an existing `ListId` and that list's current
//!    `ListUpdateToken` UPDATES it in place: same id and ARN, a rotated token,
//!    an unchanged create instant, and the superseded `App` array archived in
//!    `PreviousAppsList` under the version number it held.
//! 3. A token that is absent or stale is refused with
//!    `InvalidOperationException` and changes nothing; an id that names no
//!    customer list is `ResourceNotFoundException`.
//! 4. The write is administrator-gated and decides authorization FIRST: with no
//!    Firewall Manager administrator association the call is refused with
//!    `AccessDeniedException` and nothing is stored.
//!
//! The table in [`cases`] is the class those rules range over: the rows differ
//! in how many applications the list holds (none, one, several), in application
//! protocol form (IP protocol keyword and protocol NUMBER), in port (both
//! published `IPPortNumber` bounds and typical ports), in name length (both
//! published `ResourceName` bounds) and in whether — and how many — tags the
//! create sends. Every rule is asserted on every row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_fms_awsfms_20180101::state;

// The wire harness and the store/seeding helpers live in one place for this test
// target, so no two suites can drift in how a request is signed, how an error
// envelope is read or how a resource is seeded.
use crate::administrator_gated_operations::{associate_admin, seed_apps_list, with_store};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A second Region, to assert the ARN's Region segment comes from the request's
/// credential scope rather than from a constant.
const OTHER_REGION: &str = "ap-southeast-2";

/// A well-formed `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names nothing.
const UNKNOWN_LIST_ID: &str = "11111111-2222-3333-4444-555555555555";

/// A second well-formed `ListId`, used for a seeded Firewall Manager MANAGED
/// (default) applications list — which no API creates.
const MANAGED_LIST_ID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// Whether `value` is a `ListId` as the model defines one: exactly 36 characters
/// from `^[a-z0-9A-Z-]{36}$` (`com.amazonaws.fms#ListId`).
fn is_list_id(value: &str) -> bool {
    value.chars().count() == 36
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The wire form of one `com.amazonaws.fms#App`.
fn app(name: &str, protocol: &str, port: i64) -> Value {
    json!({ "AppName": name, "Protocol": protocol, "Port": port })
}

/// One row of the applications-list table.
struct Case {
    /// Names the row in every assertion message.
    name: &'static str,
    list_name: String,
    /// The applications the create sends, in order.
    apps: Vec<Value>,
    /// The applications a later update sends, different from `apps`.
    updated: Vec<Value>,
    /// The `TagList` the create sends, or `None` to send none at all.
    tags: Option<Value>,
}

/// The applications-list class the rules are asserted over.
///
/// `AppName` and `ListName` are `com.amazonaws.fms#ResourceName` (1..128
/// characters) and the rows include both published bounds; `Protocol` is
/// `com.amazonaws.fms#Protocol` (1..20 characters), carrying both the protocol
/// keywords its own documentation names (`tcp`, `udp`, `icmp`) and a protocol
/// NUMBER, which that documentation points at the IANA protocol-number registry
/// for (6 is TCP, 17 is UDP, 1 is ICMP); `Port` is
/// `com.amazonaws.fms#IPPortNumber`, whose published range is 0..65535, and the
/// rows include both bounds.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one application, no TagList at all",
            list_name: "web-tier".to_string(),
            apps: vec![app("https", "tcp", 443)],
            updated: vec![app("http", "tcp", 80)],
            tags: None,
        },
        Case {
            name: "several applications, keywords and protocol numbers, one tag",
            list_name: "list.with-many_apps".to_string(),
            apps: vec![
                app("dns", "udp", 53),
                app("mysql", "6", 3306),
                app("ssh", "tcp", 22),
            ],
            updated: vec![app("dns", "17", 53), app("ping", "1", 0)],
            tags: Some(json!([{ "Key": "team", "Value": "network" }])),
        },
        Case {
            name: "no applications at all, empty TagList, shortest modelled name",
            list_name: "a".to_string(),
            apps: Vec::new(),
            updated: vec![app("quic", "udp", 443)],
            tags: Some(json!([])),
        },
        Case {
            // The modelled bounds: a 128-character ResourceName, a
            // 20-character Protocol token and both ends of IPPortNumber.
            name: "longest modelled names and protocol token, both port bounds",
            list_name: "n".repeat(128),
            apps: vec![
                app(&"a".repeat(128), &"P".repeat(20), 65535),
                app("lowest-port", "1", 0),
            ],
            // Order is the caller's, not sorted by the service.
            updated: vec![app("lowest-port", "1", 0), app("plain", "tcp", 1)],
            tags: Some(json!([
                { "Key": "env", "Value": "test" },
                { "Key": "owner", "Value": "" },
            ])),
        },
    ]
}

/// The create request for a row.
fn create_body(case: &Case) -> Value {
    let mut body = json!({
        "AppsList": {
            "ListName": case.list_name,
            "AppsList": case.apps,
        },
    });
    if let Some(tags) = &case.tags {
        body["TagList"] = tags.clone();
    }
    body
}

/// Create the row's list and return `(status, body)`.
async fn create(router: &MultiServiceRouter, case: &Case) -> (u16, Value) {
    call(router, "PutAppsList", &create_body(case)).await
}

/// The update request for a row: the id, the current token and `apps`.
fn update_body(list_id: &str, list_name: &str, token: &str, apps: &[Value]) -> Value {
    json!({
        "AppsList": {
            "ListId": list_id,
            "ListName": list_name,
            "ListUpdateToken": token,
            "AppsList": apps,
        },
    })
}

/// The tags a row's create sends, as `(key, value)` pairs in order.
fn expected_tags(case: &Case) -> Vec<(String, String)> {
    case.tags
        .as_ref()
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .map(|tag| {
                    (
                        tag["Key"].as_str().unwrap_or_default().to_string(),
                        tag["Value"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 1: a create mints the identifiers, stores the caller's document and is
/// read back identically by `GetAppsList`.
#[tokio::test]
async fn a_created_apps_list_round_trips_through_get() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    let mut minted: Vec<String> = Vec::new();

    for case in cases() {
        let (status, body) = create(&router, &case).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected PutAppsList to answer 200, got {status} with {body}",
            case.name
        );

        let list = &body["AppsList"];
        let list_id = list["ListId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_list_id(&list_id),
            "case {:?}: the service must mint a modelled ListId, got {:?}",
            case.name,
            list_id
        );
        assert_eq!(
            list["ListName"].as_str(),
            Some(case.list_name.as_str()),
            "case {:?}: the stored list must report the name that was sent, got {body}",
            case.name
        );
        assert_eq!(
            list["AppsList"],
            Value::Array(case.apps.clone()),
            "case {:?}: the stored list must report the applications that were sent, in order, \
             got {body}",
            case.name
        );
        assert!(
            list["ListUpdateToken"]
                .as_str()
                .is_some_and(|token| !token.is_empty()),
            "case {:?}: a created list must carry a first ListUpdateToken, got {body}",
            case.name
        );
        assert!(
            list["CreateTime"].is_number() && list["LastUpdateTime"].is_number(),
            "case {:?}: a created list must carry both timestamps, got {body}",
            case.name
        );
        assert!(
            list.get("PreviousAppsList").is_none() || list["PreviousAppsList"] == json!({}),
            "case {:?}: a list created once has no previous version, got {body}",
            case.name
        );
        assert_eq!(
            body["AppsListArn"].as_str(),
            Some(state::apps_list_arn(REGION, &list_id).as_str()),
            "case {:?}: unexpected AppsListArn, got {body}",
            case.name
        );

        // The round trip: the reader reports exactly what the writer returned.
        let (status, got) = call(&router, "GetAppsList", &json!({ "ListId": list_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetAppsList to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got, body,
            "case {:?}: GetAppsList must report what PutAppsList wrote, got {got}",
            case.name
        );

        // A created list is a CUSTOMER list, so its id names nothing in the
        // managed (default) namespace.
        let (status, refused) = call(
            &router,
            "GetAppsList",
            &json!({ "ListId": list_id, "DefaultList": true }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ResourceNotFoundException"),
            "case {:?}: a created list is not a default list, got {status} with {refused}",
            case.name
        );

        minted.push(list_id);
    }

    // Each create is its own resource: no two rows share an id, and neither does
    // a second create of an identical document.
    let repeat = cases().remove(0);
    let (_, body) = create(&router, &repeat).await;
    minted.push(
        body["AppsList"]["ListId"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    );
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every create must mint a distinct ListId, got {minted:?}"
    );
}

/// Rule 2: an update keeps the id, the ARN and the create instant, rotates the
/// token and archives the superseded `App` array under its version number.
#[tokio::test]
async fn an_update_rotates_the_token_and_archives_the_previous_version() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    for case in cases() {
        let (_, created) = create(&router, &case).await;
        let list_id = created["AppsList"]["ListId"]
            .as_str()
            .expect("the create minted a ListId")
            .to_string();
        let first_token = created["AppsList"]["ListUpdateToken"]
            .as_str()
            .expect("the create minted a token")
            .to_string();
        let create_time = created["AppsList"]["CreateTime"].clone();

        let (status, updated) = call(
            &router,
            "PutAppsList",
            &update_body(&list_id, &case.list_name, &first_token, &case.updated),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the update to answer 200, got {status} with {updated}",
            case.name
        );
        assert_eq!(
            updated["AppsList"]["ListId"].as_str(),
            Some(list_id.as_str()),
            "case {:?}: an update must keep the list's id, got {updated}",
            case.name
        );
        assert_eq!(
            updated["AppsListArn"], created["AppsListArn"],
            "case {:?}: an update must keep the list's ARN, got {updated}",
            case.name
        );
        assert_eq!(
            updated["AppsList"]["AppsList"],
            Value::Array(case.updated.clone()),
            "case {:?}: an update must report the applications it sent, got {updated}",
            case.name
        );
        let second_token = updated["AppsList"]["ListUpdateToken"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !second_token.is_empty() && second_token != first_token,
            "case {:?}: a successful update must rotate the ListUpdateToken, got {updated}",
            case.name
        );
        assert_eq!(
            updated["AppsList"]["CreateTime"], create_time,
            "case {:?}: an update must not move the create instant, got {updated}",
            case.name
        );
        assert_eq!(
            updated["AppsList"]["PreviousAppsList"],
            json!({ "1": case.apps }),
            "case {:?}: the superseded applications array must be archived as version 1, got \
             {updated}",
            case.name
        );

        // A second update archives the version it supersedes beside the first.
        let third = vec![app("https", "tcp", 443)];
        let (status, again) = call(
            &router,
            "PutAppsList",
            &update_body(&list_id, &case.list_name, &second_token, &third),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the second update to answer 200, got {status} with {again}",
            case.name
        );
        assert_eq!(
            again["AppsList"]["PreviousAppsList"],
            json!({ "1": case.apps, "2": case.updated }),
            "case {:?}: each update must archive the version it supersedes, got {again}",
            case.name
        );

        // And the reader agrees with the writer: there is ONE stored list.
        let (status, got) = call(&router, "GetAppsList", &json!({ "ListId": list_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetAppsList to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got, again,
            "case {:?}: GetAppsList must report the updated list, got {got}",
            case.name
        );
    }
}

/// Rule 3: an update is conditional on the token, and a refused update changes
/// nothing; an id outside the customer namespace is `ResourceNotFoundException`.
#[tokio::test]
async fn an_update_is_refused_without_the_current_token_and_changes_nothing() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    // A Firewall Manager MANAGED list, which this API does not write.
    with_store(&engine, |store| {
        seed_apps_list(store, REGION, MANAGED_LIST_ID, "aws-managed", &[], true);
    });

    for case in cases() {
        let (_, created) = create(&router, &case).await;
        let list_id = created["AppsList"]["ListId"]
            .as_str()
            .expect("the create minted a ListId")
            .to_string();
        let token = created["AppsList"]["ListUpdateToken"]
            .as_str()
            .expect("the create minted a token")
            .to_string();

        // The tokens that are NOT the current one: absent, a well-formed value
        // this service never minted, and a mutation of the current one.
        let bad_tokens = [
            None,
            Some("00000000000000000000000000000000000".to_string()),
            Some(format!("{token}-stale")),
        ];
        for bad in &bad_tokens {
            let mut list = json!({
                "ListId": list_id,
                "ListName": case.list_name,
                "AppsList": [app("telnet", "tcp", 23)],
            });
            if let Some(value) = bad {
                list["ListUpdateToken"] = json!(value);
            }
            let (status, body) = call(&router, "PutAppsList", &json!({ "AppsList": list })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidOperationException"),
                "case {:?} with token {bad:?}: an update without the current ListUpdateToken must \
                 be refused, got {status} with {body}",
                case.name
            );
        }

        // Nothing moved: the list still reads back exactly as it was created.
        let (status, got) = call(&router, "GetAppsList", &json!({ "ListId": list_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetAppsList to answer 200, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got, created,
            "case {:?}: a refused update must leave the stored list untouched, got {got}",
            case.name
        );
    }

    // An id that names no customer list — one nothing was ever stored under, and
    // one naming a Firewall Manager managed list — is not found.
    for list_id in [UNKNOWN_LIST_ID, MANAGED_LIST_ID] {
        let (status, body) = call(
            &router,
            "PutAppsList",
            &update_body(
                list_id,
                "no-such-list",
                "whatever-token",
                &[app("https", "tcp", 443)],
            ),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {list_id:?}: an id naming no customer list must not be found, got {status} with \
             {body}"
        );
    }

    // The seeded managed list is untouched by those refusals.
    with_store(&engine, |store| {
        let managed = store
            .apps_lists
            .get(MANAGED_LIST_ID)
            .expect("the seeded managed list is still stored");
        assert!(
            managed.list.apps_list.is_empty() && managed.list.list_name == "aws-managed",
            "a refused update must not rewrite the managed list, got {managed:?}"
        );
    });
}

/// The ARN's Region segment is the Region the request was signed for, and a
/// create's `TagList` is what the resource carries: an update that sends no
/// `TagList` keeps those tags, and one that sends a list replaces them.
#[tokio::test]
async fn the_arn_region_and_the_tags_come_from_the_request() {
    for region in [REGION, OTHER_REGION] {
        let (router, engine) = emulator_with_engine();
        let (status, body) = call_in(
            &router,
            "AssociateAdminAccount",
            &json!({ "AdminAccount": ACCOUNT }),
            region,
        )
        .await;
        assert_eq!(
            status, 200,
            "region {region}: AssociateAdminAccount must answer 200, got {status} with {body}"
        );

        for case in cases() {
            let (status, created) =
                call_in(&router, "PutAppsList", &create_body(&case), region).await;
            assert_eq!(
                status, 200,
                "case {:?} in {region}: expected 200, got {status} with {created}",
                case.name
            );
            let list_id = created["AppsList"]["ListId"]
                .as_str()
                .expect("the create minted a ListId")
                .to_string();
            assert_eq!(
                created["AppsListArn"].as_str(),
                Some(state::apps_list_arn(region, &list_id).as_str()),
                "case {:?} in {region}: the ARN must carry the request's Region, got {created}",
                case.name
            );

            let sent_tags = expected_tags(&case);
            let arn = state::apps_list_arn(region, &list_id);
            with_store(&engine, |store| {
                let stored: Vec<(String, String)> = store
                    .apps_lists
                    .get(&list_id)
                    .expect("the created list is stored")
                    .tags
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect();
                assert_eq!(
                    stored, sent_tags,
                    "case {:?} in {region}: the list must carry the tags the create sent",
                    case.name
                );
                let indexed: Vec<(String, String)> = store
                    .tags_by_arn
                    .get(&arn)
                    .expect("the created list's ARN is in the tag index")
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect();
                assert_eq!(
                    indexed, sent_tags,
                    "case {:?} in {region}: the ARN-keyed tag index must agree with the record",
                    case.name
                );
            });

            // An update that sends no TagList keeps the tags; one that sends a
            // list replaces them.
            let token = created["AppsList"]["ListUpdateToken"]
                .as_str()
                .expect("the create minted a token")
                .to_string();
            let (status, updated) = call_in(
                &router,
                "PutAppsList",
                &update_body(&list_id, &case.list_name, &token, &case.updated),
                region,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?} in {region}: expected the update to answer 200, got {status} with \
                 {updated}",
                case.name
            );
            with_store(&engine, |store| {
                let stored: Vec<(String, String)> = store
                    .apps_lists
                    .get(&list_id)
                    .expect("the updated list is stored")
                    .tags
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect();
                assert_eq!(
                    stored, sent_tags,
                    "case {:?} in {region}: an update sending no TagList keeps the tags",
                    case.name
                );
            });

            let token = updated["AppsList"]["ListUpdateToken"]
                .as_str()
                .expect("the update rotated the token")
                .to_string();
            let mut retagging = update_body(&list_id, &case.list_name, &token, &case.updated);
            retagging["TagList"] = json!([{ "Key": "replaced", "Value": "yes" }]);
            let (status, replaced) = call_in(&router, "PutAppsList", &retagging, region).await;
            assert_eq!(
                status, 200,
                "case {:?} in {region}: expected the retagging update to answer 200, got {status} \
                 with {replaced}",
                case.name
            );
            with_store(&engine, |store| {
                let stored: Vec<(String, String)> = store
                    .apps_lists
                    .get(&list_id)
                    .expect("the retagged list is stored")
                    .tags
                    .iter()
                    .map(|tag| (tag.key.clone(), tag.value.clone()))
                    .collect();
                assert_eq!(
                    stored,
                    vec![("replaced".to_string(), "yes".to_string())],
                    "case {:?} in {region}: a TagList that IS sent replaces the tag set",
                    case.name
                );
            });
        }
    }
}

/// Rule 4: authorization is decided FIRST — with no administrator association
/// every create and update is refused and nothing is written.
#[tokio::test]
async fn without_an_administrator_the_write_is_refused_and_stores_nothing() {
    let (router, engine) = emulator_with_engine();

    for case in cases() {
        let (status, body) = create(&router, &case).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: with no administrator the create must be refused, got {status} with {body}",
            case.name
        );
        let (status, body) = call(
            &router,
            "PutAppsList",
            &update_body(UNKNOWN_LIST_ID, &case.list_name, "any-token", &case.apps),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: with no administrator the update must be refused before the id is looked \
             up, got {status} with {body}",
            case.name
        );
    }

    with_store(&engine, |store| {
        assert!(
            store.apps_lists.is_empty() && store.tags_by_arn.is_empty(),
            "a refused write must store nothing, got {:?} and {:?}",
            store.apps_lists,
            store.tags_by_arn
        );
    });
}

/// A created list is deletable, and deleting it removes it from state — so the
/// writer and the deleter act on the SAME record.
#[tokio::test]
async fn a_created_apps_list_can_be_deleted() {
    let (router, _engine) = emulator_with_engine();
    associate_admin(&router).await;

    for case in cases() {
        let (_, created) = create(&router, &case).await;
        let list_id = created["AppsList"]["ListId"]
            .as_str()
            .expect("the create minted a ListId")
            .to_string();

        let (status, body) = call(&router, "DeleteAppsList", &json!({ "ListId": list_id })).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DeleteAppsList to answer 200, got {status} with {body}",
            case.name
        );

        for operation in ["GetAppsList", "DeleteAppsList"] {
            let (status, body) = call(&router, operation, &json!({ "ListId": list_id })).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {:?}: {operation} on a deleted list must not find it, got {status} with \
                 {body}",
                case.name
            );
        }
    }
}
