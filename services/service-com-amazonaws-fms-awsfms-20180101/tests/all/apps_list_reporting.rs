// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetAppsList`: the reporting read of ONE Firewall
//! Manager applications list.
//!
//! Every test drives the service the way a client does: an awsJson1_1 `POST /`
//! through [`MultiServiceRouter::dispatch`], with the `X-Amz-Target` header and
//! a SigV4 credential scope, so the whole deserialize → handle → serialize walk
//! runs and the assertions are about the WIRE response.
//!
//! The rules under test:
//!
//! 1. `GetAppsList` reports the STORED applications list the `ListId` names —
//!    its name, its `App` array in the stored order and the service-owned
//!    `ListId`, `ListUpdateToken` and timestamps — beside the list's ARN, whose
//!    Region segment is the Region the request was signed for.
//! 2. `DefaultList` selects the namespace the id is resolved in: a Firewall
//!    Manager managed (default) list and a customer list are two namespaces, so
//!    an id resolved in the wrong one is `ResourceNotFoundException`, as is an
//!    id naming nothing or naming a resource of another kind.
//! 3. The read is REPORTING: it changes nothing a later caller sees, so
//!    repeating it answers the same document and the list is still there to be
//!    deleted afterwards.
//! 4. It is administrator-gated and decides authorization FIRST: with no
//!    Firewall Manager administrator association, a read of a list that DOES
//!    exist is refused with `AccessDeniedException` rather than reported.
//!
//! The table in [`cases`] is the class those rules range over: the rows differ
//! in how many applications the list holds (none, one, several), in application
//! protocol form (IP protocol keyword and protocol NUMBER), in port (both
//! published `IPPortNumber` bounds and a typical port), in name length (both
//! published `ResourceName` bounds), in whether the stored list carries an
//! archived previous version, and in which namespace (managed or customer) the
//! list lives. Every rule is asserted on every row.

use serde_json::{json, Value};

use service_com_amazonaws_fms_awsfms_20180101::ops::types::App;
use service_com_amazonaws_fms_awsfms_20180101::state;

// The wire harness and the store/seeding helpers live in one place for this test
// target, so no two suites can drift in how a request is signed, how an error
// envelope is read or how a resource is seeded.
use crate::administrator_gated_operations::{
    associate_admin, seed_apps_list, seed_protocols_list, with_store,
};
use crate::policy_lifecycle::{call, call_in, emulator_with_engine, error_code, ACCOUNT, REGION};

/// A second Region, to assert the reported ARN's Region segment comes from the
/// request's credential scope rather than from a constant.
const OTHER_REGION: &str = "eu-central-1";

/// A well-formed `ListId` (`com.amazonaws.fms#ListId`: 36 characters matching
/// `^[a-z0-9A-Z-]{36}$`) that names nothing at all.
const UNKNOWN_LIST_ID: &str = "77777777-6666-5555-4444-333333333333";

/// A second well-formed `ListId`, used for a seeded PROTOCOLS list — a resource
/// of another kind, whose id therefore names no applications list.
const PROTOCOLS_LIST_ID: &str = "22222222-3333-4444-5555-666666666666";

/// A `ListId` for row `index` of the table: 36 characters from
/// `^[a-z0-9A-Z-]{36}$`, as `com.amazonaws.fms#ListId` requires.
fn list_id(index: usize) -> String {
    let id = format!("apps-list-{index:0>26}");
    assert_eq!(id.chars().count(), 36, "a ListId is 36 characters: {id:?}");
    id
}

/// One application, as `com.amazonaws.fms#App` models it.
fn app(name: &str, protocol: &str, port: i64) -> App {
    App {
        app_name: name.to_string(),
        protocol: protocol.to_string(),
        port,
    }
}

/// The wire form of an `App`, for comparing against the reported array.
fn app_json(app: &App) -> Value {
    json!({ "AppName": app.app_name, "Protocol": app.protocol, "Port": app.port })
}

/// One row of the applications-list table.
struct Case {
    /// Names the row in every assertion message.
    name: &'static str,
    list_name: String,
    /// The applications the stored list holds, in stored order.
    apps: Vec<App>,
    /// Archived previous versions of the `App` array, keyed by version number
    /// (`com.amazonaws.fms#PreviousAppsList`), or empty for a list with no
    /// history.
    previous: Vec<(String, Vec<App>)>,
    /// Whether the stored list is a Firewall Manager MANAGED (default) list.
    default_list: bool,
}

/// The applications-list class the rules are asserted over.
///
/// `AppName` and `ListName` are `com.amazonaws.fms#ResourceName` (1..128
/// characters) and the rows include both published bounds; `Protocol` is
/// `com.amazonaws.fms#Protocol` (1..20 characters), carrying both the protocol
/// keywords its own documentation names (`tcp`, `udp`, `icmp`) and a protocol
/// NUMBER, which that documentation points at the IANA protocol-number registry
/// for (6 is TCP, 1 is ICMP); `Port` is `com.amazonaws.fms#IPPortNumber`, whose
/// published range is 0..65535, and the rows include both bounds.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no applications at all, shortest modelled name",
            list_name: "a".to_string(),
            apps: Vec::new(),
            previous: Vec::new(),
            default_list: false,
        },
        Case {
            name: "one application, protocol keyword",
            list_name: "web-tier".to_string(),
            apps: vec![app("https", "tcp", 443)],
            previous: Vec::new(),
            default_list: false,
        },
        Case {
            name: "several applications, keywords and protocol numbers",
            list_name: "list.with-many_apps".to_string(),
            apps: vec![
                app("dns", "udp", 53),
                // The lowest port the model allows, on ICMP named by number.
                app("ping", "1", 0),
                app("mysql", "6", 3306),
                app("ssh", "tcp", 22),
            ],
            previous: Vec::new(),
            default_list: false,
        },
        Case {
            name: "longest modelled names and protocol token, highest port",
            list_name: "n".repeat(128),
            apps: vec![
                app(&"a".repeat(128), &"P".repeat(20), 65535),
                app("plain", "tcp", 1),
            ],
            previous: Vec::new(),
            default_list: false,
        },
        Case {
            name: "customer list with two archived previous versions",
            list_name: "versioned".to_string(),
            apps: vec![app("http", "tcp", 80), app("quic", "udp", 443)],
            previous: vec![
                ("1".to_string(), vec![app("http", "tcp", 80)]),
                (
                    "2".to_string(),
                    vec![app("http", "tcp", 80), app("alt", "tcp", 8080)],
                ),
            ],
            default_list: false,
        },
        Case {
            name: "Firewall Manager managed (default) list",
            list_name: "aws-managed-apps".to_string(),
            apps: vec![app("inspect", "tcp", 8080), app("icmp-echo", "icmp", 0)],
            previous: Vec::new(),
            default_list: true,
        },
    ]
}

/// Seed the whole table into `store` under `region` and answer each row's id and
/// `ListUpdateToken`, the service-owned token a reader must report.
fn seed_cases(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    region: &str,
    cases: &[Case],
) -> Vec<(String, String)> {
    with_store(engine, |store| {
        cases
            .iter()
            .enumerate()
            .map(|(index, case)| {
                let id = list_id(index);
                let record = seed_apps_list(
                    store,
                    region,
                    &id,
                    &case.list_name,
                    &case.apps,
                    case.default_list,
                );
                if !case.previous.is_empty() {
                    let history = case
                        .previous
                        .iter()
                        .map(|(version, apps)| (version.clone(), apps.clone()))
                        .collect();
                    store
                        .apps_lists
                        .get_mut(&id)
                        .expect("the seeded list is stored")
                        .list
                        .previous_apps_list = Some(history);
                }
                (id, record.update_token)
            })
            .collect()
    })
}

/// The `GetAppsList` request for a row.
fn get_body(list_id: &str, default_list: bool) -> Value {
    json!({ "ListId": list_id, "DefaultList": default_list })
}

/// Rule 1: the read reports the stored list and its ARN, with the ARN's Region
/// segment taken from the request — asserted in two Regions.
#[tokio::test]
async fn a_stored_apps_list_is_reported_with_its_arn() {
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

        let table = cases();
        let seeded = seed_cases(&engine, region, &table);

        for (case, (id, token)) in table.iter().zip(&seeded) {
            let (status, got) =
                call_in(&router, "GetAppsList", &get_body(id, case.default_list), region).await;
            assert_eq!(
                status, 200,
                "case {:?} in {region}: expected GetAppsList to answer 200, got {status} with \
                 {got}",
                case.name
            );

            let list = &got["AppsList"];
            assert_eq!(
                list["ListId"].as_str(),
                Some(id.as_str()),
                "case {:?} in {region}: the list must report its own id, got {got}",
                case.name
            );
            assert_eq!(
                list["ListName"].as_str(),
                Some(case.list_name.as_str()),
                "case {:?} in {region}: the list must report its stored name, got {got}",
                case.name
            );
            let expected_apps: Vec<Value> = case.apps.iter().map(app_json).collect();
            assert_eq!(
                list["AppsList"],
                Value::Array(expected_apps),
                "case {:?} in {region}: the list must report its stored applications, in order, \
                 got {got}",
                case.name
            );
            assert_eq!(
                list["ListUpdateToken"].as_str(),
                Some(token.as_str()),
                "case {:?} in {region}: the list must report the stored ListUpdateToken, got {got}",
                case.name
            );
            assert!(
                list["CreateTime"].is_number() && list["LastUpdateTime"].is_number(),
                "case {:?} in {region}: the list must report both timestamps, got {got}",
                case.name
            );
            if case.previous.is_empty() {
                assert!(
                    list.get("PreviousAppsList").is_none() || list["PreviousAppsList"] == json!({}),
                    "case {:?} in {region}: a list with no history must report none, got {got}",
                    case.name
                );
            } else {
                let expected_history: Value = Value::Object(
                    case.previous
                        .iter()
                        .map(|(version, apps)| {
                            (
                                version.clone(),
                                Value::Array(apps.iter().map(app_json).collect()),
                            )
                        })
                        .collect(),
                );
                assert_eq!(
                    list["PreviousAppsList"], expected_history,
                    "case {:?} in {region}: the list must report its archived versions, got {got}",
                    case.name
                );
            }
            assert_eq!(
                got["AppsListArn"].as_str(),
                Some(state::apps_list_arn(region, id).as_str()),
                "case {:?} in {region}: unexpected AppsListArn, got {got}",
                case.name
            );
        }
    }
}

/// Rule 2: the id is resolved in the namespace `DefaultList` names, and an id
/// naming nothing — or a resource of another kind — is not found.
#[tokio::test]
async fn an_id_outside_the_requested_namespace_is_not_found() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let table = cases();
    let seeded = seed_cases(&engine, REGION, &table);
    // A resource of ANOTHER kind, whose id is well formed for this operation.
    with_store(&engine, |store| {
        seed_protocols_list(store, PROTOCOLS_LIST_ID, "a-protocols-list", &["TCP"], false);
    });

    for (case, (id, _)) in table.iter().zip(&seeded) {
        // The wrong namespace for this row's list.
        let (status, body) = call(&router, "GetAppsList", &get_body(id, !case.default_list)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {:?}: a list is reported only in its own namespace (DefaultList={}), got \
             {status} with {body}",
            case.name,
            !case.default_list
        );
    }

    // An id naming nothing, and one naming a protocols list, in BOTH namespaces.
    for id in [UNKNOWN_LIST_ID, PROTOCOLS_LIST_ID] {
        for default_list in [false, true] {
            let (status, body) = call(&router, "GetAppsList", &get_body(id, default_list)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {id:?} (DefaultList={default_list}): an id naming no applications list must \
                 not be found, got {status} with {body}"
            );
            assert_eq!(
                body["Message"].as_str(),
                Some(state::RESOURCE_NOT_FOUND_MESSAGE),
                "case {id:?} (DefaultList={default_list}): the not-found must carry this \
                 service's message, got {body}"
            );
        }
    }

    // A request with no DefaultList member at all is the modelled default
    // (false), so a managed list is still not reported.
    let managed = table
        .iter()
        .zip(&seeded)
        .find(|(case, _)| case.default_list)
        .map(|(_, (id, _))| id.clone())
        .expect("the table holds a managed list");
    let (status, body) = call(&router, "GetAppsList", &json!({ "ListId": managed })).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "with DefaultList defaulted to false a managed list must not be reported, got {status} \
         with {body}"
    );
}

/// Rule 3: the read reports and does not mutate — a repeat answers the same
/// document, nothing in the store moves, and the list is still there to delete.
#[tokio::test]
async fn reading_a_list_changes_nothing_a_later_caller_sees() {
    let (router, engine) = emulator_with_engine();
    associate_admin(&router).await;

    let table = cases();
    let seeded = seed_cases(&engine, REGION, &table);
    let before = with_store(&engine, |store| format!("{:?}", store.apps_lists));

    for (case, (id, _)) in table.iter().zip(&seeded) {
        let (status, first) =
            call(&router, "GetAppsList", &get_body(id, case.default_list)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetAppsList to answer 200, got {status} with {first}",
            case.name
        );
        let (status, second) =
            call(&router, "GetAppsList", &get_body(id, case.default_list)).await;
        assert_eq!(
            (status, &second),
            (200, &first),
            "case {:?}: a repeated read must report the same document, got {status} with {second}",
            case.name
        );
    }

    let after = with_store(&engine, |store| format!("{:?}", store.apps_lists));
    assert_eq!(
        after, before,
        "a reporting read must leave the stored applications lists untouched"
    );

    // And the list is still a live resource: it can be deleted, after which the
    // read no longer reports it while its siblings still are.
    let (customer_id, _) = table
        .iter()
        .zip(&seeded)
        .find(|(case, _)| !case.default_list)
        .map(|(case, (id, _))| (id.clone(), case.name))
        .expect("the table holds a customer list");
    let (status, body) = call(&router, "DeleteAppsList", &json!({ "ListId": customer_id })).await;
    assert_eq!(
        status, 200,
        "expected DeleteAppsList to answer 200, got {status} with {body}"
    );
    let (status, body) = call(&router, "GetAppsList", &get_body(&customer_id, false)).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "a deleted list must no longer be reported, got {status} with {body}"
    );
    for (case, (id, _)) in table.iter().zip(&seeded) {
        if *id == customer_id {
            continue;
        }
        let (status, got) = call(&router, "GetAppsList", &get_body(id, case.default_list)).await;
        assert_eq!(
            status, 200,
            "case {:?}: deleting one list must not remove another, got {status} with {got}",
            case.name
        );
    }
}

/// Rule 4: authorization is decided FIRST — with no administrator association a
/// read of a list that exists is refused, and the refusal reveals nothing about
/// the store and writes nothing to it.
#[tokio::test]
async fn without_an_administrator_the_read_is_refused_before_the_lookup() {
    let (router, engine) = emulator_with_engine();
    let table = cases();
    let seeded = seed_cases(&engine, REGION, &table);

    for (case, (id, _)) in table.iter().zip(&seeded) {
        let (status, body) = call(&router, "GetAppsList", &get_body(id, case.default_list)).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AccessDeniedException"),
            "case {:?}: with no administrator the read of an existing list must be refused, got \
             {status} with {body}",
            case.name
        );
    }
    // The same refusal for an id that names nothing: the answer does not leak
    // whether the resource exists.
    let (status, body) = call(&router, "GetAppsList", &get_body(UNKNOWN_LIST_ID, false)).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "AccessDeniedException"),
        "an unauthorized read must not leak that the id names nothing, got {status} with {body}"
    );

    // Nothing was written or removed, and once an administrator IS associated
    // every seeded list reads back.
    associate_admin(&router).await;
    for (case, (id, token)) in table.iter().zip(&seeded) {
        let (status, got) = call(&router, "GetAppsList", &get_body(id, case.default_list)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the refused reads must have left the list stored, got {status} with {got}",
            case.name
        );
        assert_eq!(
            got["AppsList"]["ListUpdateToken"].as_str(),
            Some(token.as_str()),
            "case {:?}: a refused read must not rotate the ListUpdateToken, got {got}",
            case.name
        );
    }
}
