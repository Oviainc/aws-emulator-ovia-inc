// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateExtension`, the `PATCH /extensions/{ExtensionIdentifier}`
//! editor of an extension version this account created.
//!
//! Every case drives a real HTTP request through the router the emulator's front door
//! uses, so what is asserted is what an AppConfig client sees on the wire. The rules
//! under test:
//!
//! * a `ParameterMap` an update would store is refused with HTTP 400
//!   `BadRequestException`, naming the offending parameter keys, when it leaves ANY
//!   parameter both `Required` and `Dynamic` — "Dynamic parameters can't be marked
//!   Required" (`smithy_query(query="doc",shape="Parameter",member="Dynamic")`) — and the
//!   refused update stores nothing, so the parameters keep the state they had;
//! * the update is IN PLACE on the extension version the request addresses: the version
//!   number, the ARN naming it and every member the request omits are kept, no version is
//!   forked, every other version of the extension is untouched, and `GetExtension`
//!   reports exactly what the update answered;
//! * `VersionNumber` names WHICH version to edit, not an optimistic lock: N callers
//!   supplying the same current version number all land, the extension ends carrying one
//!   of them, and a later serial update still lands;
//! * an identifier addressing no extension version this account holds is refused 404, and
//!   an extension AppConfig itself authors — which no account owns — is refused 400.
//!
//! The tables span what each rule generalises over rather than one extension: each
//! combination of a parameter's two flags, one offending parameter among several and
//! several at once, maps of one up to the ten the `ParameterMap` bound allows, which
//! members an edit carries (description, actions, parameters, all three, none), which
//! identifier form addresses the extension (id, name, version ARN), and whether the
//! version is pinned by the request, by the ARN or left to the highest.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state::{
    AWS_AUTHORED_EXTENSIONS, AWS_AUTHORED_EXTENSION_VERSION,
};

use crate::appconfig_workflow::{json_call, router, Answer};
use crate::extensions::{actions, create_extension, extension_facts, get_extension, minted_id};
use crate::resource_tags::{arn, pct};

/// The most parameters one extension declares (`ParameterMap, len 1..10`).
const MAX_PARAMETERS: usize = 10;

/// How many callers edit one extension version at the same time. More than two, so the
/// test is about ANY number of concurrent editors rather than a single pair.
const RACERS: usize = 6;

/// `PATCH /extensions/{ExtensionIdentifier}`. The identifier is bound to a NON-greedy
/// `@httpLabel`, so an ARN travels percent-encoded; every other member of
/// `UpdateExtensionRequest`, `VersionNumber` included, travels in the body. `pub(crate)`
/// because more than one suite writes an extension through the same front door.
pub(crate) async fn update_extension(
    router: &MultiServiceRouter,
    identifier: &str,
    body: &Value,
) -> Answer {
    json_call(
        router,
        "PATCH",
        &format!("/extensions/{}", pct(identifier)),
        body,
    )
    .await
}

/// A `ParameterMap` holding `names`, the `index`-th carrying the flags `flags` gives it.
fn parameters(names: &[String], flags: impl Fn(usize) -> (bool, bool)) -> Value {
    Value::Object(
        names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let (required, dynamic) = flags(index);
                (
                    name.clone(),
                    json!({
                        "Description": format!("parameter {name}"),
                        "Required": required,
                        "Dynamic": dynamic,
                    }),
                )
            })
            .collect::<Map<String, Value>>(),
    )
}

/// `count` parameter names, `P1`..`P<count>`.
fn names(count: usize) -> Vec<String> {
    (1..=count).map(|index| format!("P{index}")).collect()
}

/// The error code of a refusal, which a client matches the modeled error by.
fn error_code(answer: &Answer) -> String {
    answer.error_code()
}

/// The `Message` of a refusal body.
fn message(answer: &Answer) -> String {
    answer
        .string("Message")
        .or_else(|| answer.string("message"))
        .unwrap_or_else(|| String::from_utf8_lossy(&answer.body).to_string())
}

/// Create one extension holding a single version whose parameters are all valid, and
/// answer its id.
async fn fixture(router: &MultiServiceRouter, name: &str, parameter_count: usize) -> String {
    let body = json!({
        "Name": name,
        "Description": "before",
        "Actions": actions(&["ON_DEPLOYMENT_COMPLETE"]),
        "Parameters": parameters(&names(parameter_count), |_| (false, false)),
    });
    let created = create_extension(router, &body, None).await;
    assert_eq!(
        created.status,
        201,
        "fixture {name:?}: CreateExtension expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    minted_id(name, &created.json())
}

/// One row of the parameter-schema table: the `ParameterMap` an update offers, and the
/// parameter keys (if any) that cannot hold their flags together.
struct Schema {
    case: &'static str,
    parameters: Value,
    offending: Vec<&'static str>,
}

/// RULE: an update whose `ParameterMap` would leave a parameter both `Required` and
/// `Dynamic` is refused with HTTP 400 `BadRequestException` naming the offending keys and
/// stores nothing; every other combination of the two flags is stored.
///
/// The rows span each combination of the two flags, one offending parameter among several
/// and several offending at once, and map sizes from one up to the ten the `ParameterMap`
/// bound allows.
#[tokio::test]
async fn an_update_cannot_leave_a_parameter_both_required_and_dynamic() {
    let one = names(1);
    let three = names(3);
    let ten = names(MAX_PARAMETERS);
    let table = vec![
        Schema {
            case: "one parameter, neither flag",
            parameters: parameters(&one, |_| (false, false)),
            offending: Vec::new(),
        },
        Schema {
            case: "one parameter, required only",
            parameters: parameters(&one, |_| (true, false)),
            offending: Vec::new(),
        },
        Schema {
            case: "one parameter, dynamic only",
            parameters: parameters(&one, |_| (false, true)),
            offending: Vec::new(),
        },
        Schema {
            case: "one parameter, required and dynamic",
            parameters: parameters(&one, |_| (true, true)),
            offending: vec!["P1"],
        },
        Schema {
            case: "three parameters, the middle one required and dynamic",
            parameters: parameters(&three, |index| (index == 1, index == 1)),
            offending: vec!["P2"],
        },
        Schema {
            case: "three parameters, two of them required and dynamic",
            parameters: parameters(&three, |index| (index != 1, index != 1)),
            offending: vec!["P1", "P3"],
        },
        Schema {
            case: "the ten parameters the ParameterMap bound allows, the last one both",
            parameters: parameters(&ten, |index| {
                (index + 1 == MAX_PARAMETERS, index + 1 == MAX_PARAMETERS)
            }),
            offending: vec!["P10"],
        },
    ];

    for row in &table {
        let case = row.case;
        let router = router();
        // The extension starts with two parameters carrying no flag at all, so a refused
        // update is visible as the parameters it did NOT change.
        let before = parameters(&names(2), |_| (false, false));
        let id = fixture(&router, "schema-under-test", 2).await;

        let answer = update_extension(&router, &id, &json!({ "Parameters": row.parameters })).await;
        let expected_status = if row.offending.is_empty() { 200 } else { 400 };
        assert_eq!(
            answer.status,
            expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        let read = get_extension(&router, &id, None).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: GetExtension expected 200, got {} / {}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        let read_body = read.json();

        if row.offending.is_empty() {
            assert_eq!(
                answer.json().get("Parameters").cloned(),
                Some(row.parameters.clone()),
                "case {case:?}: expected the accepted Parameters back, got {}",
                answer.json()
            );
            assert_eq!(
                read_body.get("Parameters").cloned(),
                Some(row.parameters.clone()),
                "case {case:?}: GetExtension reported {:?} rather than the stored parameters",
                read_body.get("Parameters")
            );
        } else {
            assert_eq!(
                error_code(&answer),
                "BadRequestException",
                "case {case:?}: expected a BadRequestException, got {}",
                String::from_utf8_lossy(&answer.body)
            );
            let expected_message = format!(
                "Extension parameters {} can not be both required and dynamic",
                row.offending.join(",")
            );
            assert_eq!(
                message(&answer),
                expected_message,
                "case {case:?}: expected the message {expected_message:?}, got {:?}",
                message(&answer)
            );
            assert_eq!(
                read_body.get("Parameters").cloned(),
                Some(before.clone()),
                "case {case:?}: a refused update changed the stored parameters to {:?}",
                read_body.get("Parameters")
            );
            assert_eq!(
                read_body.get("Description").and_then(Value::as_str),
                Some("before"),
                "case {case:?}: a refused update changed the stored description: {read_body}"
            );
        }
        assert_eq!(
            read_body.get("VersionNumber").and_then(Value::as_i64),
            Some(1),
            "case {case:?}: no update of version 1 may fork a version: {read_body}"
        );
    }
}

/// How a row of the in-place table addresses the extension.
enum Address {
    /// The extension's minted id.
    Id,
    /// The extension's name.
    Name,
    /// The ARN of one version, which pins that version.
    VersionArn(i32),
}

/// One row of the in-place table: which version an edit addresses, how, and what it
/// carries.
struct Edit {
    case: &'static str,
    address: Address,
    version_number: Option<i32>,
    description: Option<&'static str>,
    actions: Option<Value>,
    parameters: Option<Value>,
    /// The version number the edit must land on.
    target: i32,
}

/// RULE: `UpdateExtension` edits the extension version its request addresses IN PLACE —
/// the version number, the ARN naming it and every member the request omits are kept, no
/// version is forked, and the extension's other versions are untouched — and
/// `GetExtension` reports exactly what the update answered.
///
/// The rows span which members the edit carries, which identifier form addresses the
/// extension, and whether the version is pinned by the request, by the ARN or left to the
/// highest one.
#[tokio::test]
async fn an_update_edits_the_addressed_version_in_place() {
    let table = vec![
        Edit {
            case: "description only, no version named: the highest version",
            address: Address::Id,
            version_number: None,
            description: Some("edited"),
            actions: None,
            parameters: None,
            target: 2,
        },
        Edit {
            case: "actions replaced on version 1, named by the request",
            address: Address::Id,
            version_number: Some(1),
            description: None,
            actions: Some(actions(&["PRE_START_DEPLOYMENT", "ON_DEPLOYMENT_STEP"])),
            parameters: None,
            target: 1,
        },
        Edit {
            case: "parameters replaced on version 1, addressed by name",
            address: Address::Name,
            version_number: Some(1),
            description: None,
            actions: None,
            parameters: Some(parameters(&names(3), |index| (index == 0, index == 2))),
            target: 1,
        },
        Edit {
            case: "every member at once, addressed by the ARN of version 1",
            address: Address::VersionArn(1),
            version_number: None,
            description: Some(""),
            actions: Some(actions(&["AT_DEPLOYMENT_TICK"])),
            parameters: Some(parameters(&names(1), |_| (true, false))),
            target: 1,
        },
        Edit {
            case: "no member at all: the version is answered unchanged",
            address: Address::Id,
            version_number: Some(2),
            description: None,
            actions: None,
            parameters: None,
            target: 2,
        },
    ];

    for row in &table {
        let case = row.case;
        let router = router();
        let name = "edited-in-place";

        // Two versions, each with its own definition, so an edit that forked a version or
        // touched the wrong one is visible.
        let first = create_extension(
            &router,
            &json!({
                "Name": name,
                "Description": "version one",
                "Actions": actions(&["ON_DEPLOYMENT_COMPLETE"]),
                "Parameters": parameters(&names(2), |_| (false, false)),
            }),
            None,
        )
        .await;
        assert_eq!(
            first.status,
            201,
            "case {case:?}: CreateExtension expected 201, got {} / {}",
            first.status,
            String::from_utf8_lossy(&first.body)
        );
        let id = minted_id(case, &first.json());
        let second = create_extension(
            &router,
            &json!({
                "Name": name,
                "Description": "version two",
                "Actions": actions(&["ON_DEPLOYMENT_BAKING"]),
                "Parameters": parameters(&names(1), |_| (true, false)),
            }),
            Some(1),
        )
        .await;
        assert_eq!(
            second.status,
            201,
            "case {case:?}: the second CreateExtension expected 201, got {} / {}",
            second.status,
            String::from_utf8_lossy(&second.body)
        );

        // What each version reads as before the edit, which is also what every version the
        // edit does NOT address must still read as afterwards.
        let mut before: BTreeMap<i32, BTreeMap<&'static str, Value>> = BTreeMap::new();
        for version in [1, 2] {
            let read = get_extension(&router, &id, Some(version)).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: GetExtension at version {version} expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            before.insert(version, extension_facts(&read.json()));
        }

        let mut body = Map::new();
        if let Some(description) = row.description {
            body.insert("Description".to_string(), Value::from(description));
        }
        if let Some(actions) = &row.actions {
            body.insert("Actions".to_string(), actions.clone());
        }
        if let Some(parameters) = &row.parameters {
            body.insert("Parameters".to_string(), parameters.clone());
        }
        if let Some(version_number) = row.version_number {
            body.insert("VersionNumber".to_string(), Value::from(version_number));
        }
        let identifier = match row.address {
            Address::Id => id.clone(),
            Address::Name => name.to_string(),
            Address::VersionArn(version) => arn(&format!("extension/{id}/{version}")),
        };

        let answer = update_extension(&router, &identifier, &Value::Object(body)).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: UpdateExtension expected 200, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // The edited version: the members the request carried, and the ones it omitted
        // exactly as they were — id, name, version number and ARN included.
        let mut expected = before
            .get(&row.target)
            .expect("the target version was read before the edit")
            .clone();
        if let Some(description) = row.description {
            expected.insert("Description", Value::from(description));
        }
        if let Some(actions) = &row.actions {
            expected.insert("Actions", actions.clone());
        }
        if let Some(parameters) = &row.parameters {
            expected.insert("Parameters", parameters.clone());
        }
        assert_eq!(
            extension_facts(&answer.json()),
            expected,
            "case {case:?}: UpdateExtension answered something other than the edited version"
        );

        // Every version reads back as the edit left it: the target edited, the other one
        // untouched, and no third version minted.
        for version in [1, 2] {
            let read = get_extension(&router, &id, Some(version)).await;
            assert_eq!(
                read.status,
                200,
                "case {case:?}: GetExtension at version {version} expected 200, got {} / {}",
                read.status,
                String::from_utf8_lossy(&read.body)
            );
            let wanted = if version == row.target {
                expected.clone()
            } else {
                before
                    .get(&version)
                    .expect("every version was read before the edit")
                    .clone()
            };
            assert_eq!(
                extension_facts(&read.json()),
                wanted,
                "case {case:?}: version {version} does not read as the edit left it"
            );
        }
        let forked = get_extension(&router, &id, Some(3)).await;
        assert_eq!(
            forked.status,
            404,
            "case {case:?}: an in-place update must fork no version, got {} / {}",
            forked.status,
            String::from_utf8_lossy(&forked.body)
        );
    }
}

/// RULE: `VersionNumber` names which version to edit rather than locking it, so N callers
/// supplying the extension's current version number all land with that same version
/// number, the extension ends carrying exactly one of them, and a later serial update
/// still lands.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_all_land_and_the_last_writer_wins() {
    let router = Arc::new(router());
    let id = fixture(&router, "raced-extension", 1).await;

    let mut racers = Vec::new();
    for racer in 0..RACERS {
        let router = Arc::clone(&router);
        let identifier = id.clone();
        racers.push(tokio::spawn(async move {
            let body = json!({
                "VersionNumber": 1,
                "Description": format!("racer-{racer}"),
            });
            (racer, update_extension(&router, &identifier, &body).await)
        }));
    }

    let mut landed = Vec::new();
    for racer in racers {
        let (index, answer) = racer.await.expect("the racing update task ran");
        assert_eq!(
            answer.status,
            200,
            "racer {index}: every concurrent update must land, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        let body = answer.json();
        assert_eq!(
            body.get("VersionNumber").and_then(Value::as_i64),
            Some(1),
            "racer {index}: an in-place update keeps the version number, got {body}"
        );
        landed.push(format!("racer-{index}"));
    }
    assert_eq!(
        landed.len(),
        RACERS,
        "expected all {RACERS} racers to land, got {landed:?}"
    );

    // One racer's description, never a blend of two, and still one single version.
    let read = get_extension(&router, &id, None).await;
    assert_eq!(
        read.status,
        200,
        "GetExtension after the race expected 200, got {} / {}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let read_body = read.json();
    assert_eq!(
        read_body.get("VersionNumber").and_then(Value::as_i64),
        Some(1),
        "the race may not fork a version, got {read_body}"
    );
    let winner = read_body
        .get("Description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        landed.contains(&winner),
        "expected the stored description to be one of {landed:?}, got {winner:?}"
    );
    let forked = get_extension(&router, &id, Some(2)).await;
    assert_eq!(
        forked.status,
        404,
        "the race may not fork a version, got {} / {}",
        forked.status,
        String::from_utf8_lossy(&forked.body)
    );

    // A serial update after the race still lands, and an independent read reflects it.
    let serial = update_extension(
        &router,
        &id,
        &json!({ "VersionNumber": 1, "Description": "after the race" }),
    )
    .await;
    assert_eq!(
        serial.status,
        200,
        "the serial update after the race expected 200, got {} / {}",
        serial.status,
        String::from_utf8_lossy(&serial.body)
    );
    let after = get_extension(&router, &id, None).await;
    assert_eq!(
        after.json().get("Description").and_then(Value::as_str),
        Some("after the race"),
        "the serial update after the race is not visible to a reader: {}",
        after.json()
    );
}

/// One row of the refusal table: an identifier and version an update cannot edit.
struct Refusal {
    case: &'static str,
    /// `None` names the fixture extension's own id.
    identifier: Option<String>,
    version_number: Option<i32>,
    status: u16,
    code: &'static str,
}

/// RULE: an `ExtensionIdentifier` (with the version the request or its ARN pins)
/// addressing no extension version this account holds is refused HTTP 404
/// `ResourceNotFoundException`, and an extension AppConfig itself authors — published to
/// every account and owned by none — is refused HTTP 400 `BadRequestException`.
///
/// The rows span an identifier nothing answers to, a version past the highest one, a
/// version ARN naming a version that was never written, and every published AWS-authored
/// extension.
#[tokio::test]
async fn an_update_of_what_this_account_cannot_edit_is_refused() {
    let router = router();
    let id = fixture(&router, "refusal-fixture", 1).await;

    let mut table = vec![
        Refusal {
            case: "an identifier no extension answers to",
            identifier: Some("no-such-extension".to_string()),
            version_number: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a version past the highest one written",
            identifier: None,
            version_number: Some(7),
            status: 404,
            code: "ResourceNotFoundException",
        },
        Refusal {
            case: "a version ARN naming a version that was never written",
            identifier: Some(arn(&format!("extension/{id}/4"))),
            version_number: None,
            status: 404,
            code: "ResourceNotFoundException",
        },
    ];
    // Every published AWS-authored extension, read from the catalogue the service serves
    // them from, so a row cannot drift from it by being hand-copied.
    for authored in AWS_AUTHORED_EXTENSIONS {
        table.push(Refusal {
            case: "an AWS-authored extension, which no account owns",
            identifier: Some(authored.name.to_string()),
            version_number: Some(AWS_AUTHORED_EXTENSION_VERSION),
            status: 400,
            code: "BadRequestException",
        });
    }

    for row in &table {
        let case = row.case;
        let identifier = row.identifier.clone().unwrap_or_else(|| id.clone());
        let mut body = Map::new();
        body.insert("Description".to_string(), Value::from("refused"));
        if let Some(version_number) = row.version_number {
            body.insert("VersionNumber".to_string(), Value::from(version_number));
        }
        let answer = update_extension(&router, &identifier, &Value::Object(body)).await;
        assert_eq!(
            answer.status,
            row.status,
            "case {case:?} ({identifier:?}): expected HTTP {}, got {} / {}",
            row.status,
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            error_code(&answer),
            row.code,
            "case {case:?} ({identifier:?}): expected a {}, got {}",
            row.code,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // Nothing any of those refusals named changed the extension they were aimed at.
    let read = get_extension(&router, &id, None).await;
    assert_eq!(
        read.json().get("Description").and_then(Value::as_str),
        Some("before"),
        "a refused update changed the stored extension: {}",
        read.json()
    );
}
