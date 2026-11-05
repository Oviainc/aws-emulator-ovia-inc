// SPDX-License-Identifier: CC0-1.0

//! What FIS's two SERVICE-owned reference catalogs serve, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler and serialization — the same path a client takes. The rule
//! under test spans both catalogs and both ways of reading each of them:
//!
//! * the action catalog (`ListActions`, `GetAction`) and the target-resource-type catalog
//!   (`ListTargetResourceTypes`, `GetTargetResourceType`) serve EVERY entry the service
//!   publishes, with that entry's published description and its published contract — an
//!   action's parameters (with each `required` flag) and the target resource types it
//!   supports, a resource type's parameters — so a client discovers the entry it would
//!   discover against the service itself, at the index the service puts it at;
//! * the ARN reported for a catalogued entry is a COMPLETE ARN:
//!   `arn:aws:fis:<region>:<account>:action/<id>`, naming the region the request was signed
//!   for and the account serving the request, so a client that parses it to recover the
//!   owning account reads one;
//! * the list and the point read of one catalog agree, entry for entry, so a read of the
//!   entry a list reports at a given index answers that same entry;
//! * the two catalogs agree with each other: every resource type a catalogued action names
//!   as a target is a type the resource-type catalog publishes;
//! * an id neither catalog publishes is refused with `ResourceNotFoundException` (HTTP 404)
//!   carrying the one fixed message, which never echoes the requested id.
//!
//! SOURCE for the expected rows below: the AWS FIS actions reference
//! (<https://docs.aws.amazon.com/fis/latest/userguide/fis-actions-reference.html>), which
//! the `Action` shape's own model documentation points at, and which publishes both the
//! actions and the resource types those actions target. The rows are written out here
//! rather than imported from `src/state.rs`, so a wrong entry in that table fails this
//! suite instead of agreeing with it. They are the entries the catalogs did NOT hold, plus
//! entries no issue cited, so the rule is asserted over a class rather than over one case.
//!
//! The HTTP harness below is local to this suite, as each sibling suite's is.

use std::collections::BTreeSet;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account every ARN this emulator reports names, spelled out here rather than
/// imported so a changed account field fails this suite.
const ACCOUNT_ID: &str = "000000000000";

/// The region an ARN names when the request's credential scope offers none usable.
const FALLBACK_REGION: &str = "us-east-1";

/// The page size the whole of either catalog fits in: `maxResults`' published ceiling
/// (`@range(min: 1, max: 100)`).
const MAX_RESULTS_CEILING: i32 = 100;

/// The message FIS answers for an action id its published catalog holds no action for —
/// fixed text naming the member, with no variable part.
const ACTION_NOT_FOUND: &str = "Invalid actionId.";

/// A SigV4 credential scope naming this service, for `region`. The service field is what
/// routes the request to fis; the region field is what a reported ARN names.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/fis/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
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

    /// The error message the answer carries, or the empty string when it carries none.
    fn message(&self) -> &str {
        self.body["message"].as_str().unwrap_or_default()
    }
}

/// One GET against this service, signed for `region`.
async fn get(router: &MultiServiceRouter, uri: &str, region: &str) -> Answer {
    let request = ::http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "fis.us-east-1.amazonaws.com")
        .header("authorization", signed_for(region))
        .header("content-type", "application/json")
        .body(Bytes::new())
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "GET {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// The `action` `GetAction` reports for `id`, in `region`.
async fn get_action(router: &MultiServiceRouter, id: &str, region: &str) -> Answer {
    get(router, &format!("/actions/{id}"), region).await
}

/// The `targetResourceType` `GetTargetResourceType` reports for `resource_type`.
async fn get_resource_type(router: &MultiServiceRouter, resource_type: &str) -> Answer {
    get(
        router,
        &format!("/targetResourceTypes/{resource_type}"),
        FALLBACK_REGION,
    )
    .await
}

/// Every element of a single-page listing of `path`, under the member `member`.
async fn list(
    router: &MultiServiceRouter,
    path: &str,
    member: &str,
    region: &str,
) -> Vec<Value> {
    let answer = get(
        router,
        &format!("{path}?maxResults={MAX_RESULTS_CEILING}"),
        region,
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {path:?}: expected HTTP 200 from the listing, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.body["nextToken"].is_null(),
        "case {path:?}: the whole catalog fits one page of {MAX_RESULTS_CEILING}, so no \
         nextToken is expected, got {:?}",
        answer.body["nextToken"]
    );
    answer.body[member]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {path:?}: expected {member:?} to be a list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// Every action `ListActions` publishes, in the order it publishes them.
async fn list_actions(router: &MultiServiceRouter, region: &str) -> Vec<Value> {
    list(router, "/actions", "actions", region).await
}

/// Every resource type `ListTargetResourceTypes` publishes, in the order it publishes them.
async fn list_resource_types(router: &MultiServiceRouter) -> Vec<Value> {
    list(
        router,
        "/targetResourceTypes",
        "targetResourceTypes",
        FALLBACK_REGION,
    )
    .await
}

/// One published ACTION the rule is asserted over: its id, the exact description the
/// reference publishes for it where that text is known verbatim, its parameters as
/// (name, required) pairs and its targets as (key, resource type) pairs.
struct ActionRow {
    id: &'static str,
    description: Option<&'static str>,
    parameters: &'static [(&'static str, bool)],
    targets: &'static [(&'static str, &'static str)],
}

/// The action rows.
///
/// They differ along what the rule generalises over: the AWS service the id belongs to, how
/// many parameters the action takes (none, one, two) and whether they are required, and how
/// many targets it supports (none, one) under which key. The ARC row is the entry the
/// catalog did not hold at all; the EC2 row is the one whose published description text the
/// recorded case pins; the remaining rows are actions no issue cited.
fn action_rows() -> Vec<ActionRow> {
    vec![
        ActionRow {
            id: "aws:arc:start-zonal-autoshift",
            description: None,
            parameters: &[("availabilityZoneIdentifier", true), ("duration", true)],
            targets: &[(
                "ManagedResources",
                "aws:arc:zonal-shift-managed-resource",
            )],
        },
        ActionRow {
            id: "aws:ec2:reboot-instances",
            description: Some("Reboot the specified EC2 instances."),
            parameters: &[],
            targets: &[("Instances", "aws:ec2:instance")],
        },
        ActionRow {
            id: "aws:ebs:pause-volume-io",
            description: None,
            parameters: &[("duration", true)],
            targets: &[("Volumes", "aws:ec2:ebs-volume")],
        },
        ActionRow {
            id: "aws:ssm:start-automation-execution",
            description: None,
            parameters: &[
                ("documentArn", true),
                ("documentParameters", false),
                ("documentVersion", false),
                ("maxConcurrency", false),
                ("maxErrors", false),
            ],
            targets: &[],
        },
    ]
}

/// One published TARGET RESOURCE TYPE the rule is asserted over: its id, the exact
/// description the reference publishes for it where that text is known verbatim, and its
/// parameters as (name, required) pairs.
struct TypeRow {
    id: &'static str,
    description: Option<&'static str>,
    parameters: &'static [(&'static str, bool)],
}

/// The resource-type rows.
///
/// The first three are the entries the catalog did not hold; they differ along how many
/// parameters a type takes (two, none) and whether they are required. The last two are
/// types no issue cited, one of them with a parameter contract.
fn type_rows() -> Vec<TypeRow> {
    vec![
        TypeRow {
            id: "aws:arc:zonal-shift-managed-resource",
            description: Some("An ARC Zonal Shift Managed Resource."),
            parameters: &[("managedResourceTypes", true), ("zonalAutoshiftStatus", false)],
        },
        TypeRow {
            id: "aws:directconnect:virtual-interface",
            description: None,
            parameters: &[],
        },
        TypeRow {
            id: "aws:dsql:cluster",
            description: None,
            parameters: &[],
        },
        TypeRow {
            id: "aws:dynamodb:global-table",
            description: None,
            parameters: &[],
        },
        TypeRow {
            id: "aws:ecs:task",
            description: None,
            parameters: &[("cluster", true), ("service", true)],
        },
    ]
}

/// The (name, required) pairs a reported `parameters` map carries, sorted by name.
fn parameter_pairs(case: &str, reported: &Value) -> Vec<(String, bool)> {
    let map = reported["parameters"].as_object().unwrap_or_else(|| {
        panic!("case {case:?}: expected a parameters map, got {reported:?}")
    });
    let mut pairs: Vec<(String, bool)> = map
        .iter()
        .map(|(name, parameter)| {
            let required = parameter["required"].as_bool().unwrap_or_else(|| {
                panic!("case {case:?}: parameter {name:?} carries no required flag: {parameter:?}")
            });
            assert!(
                parameter["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {case:?}: expected parameter {name:?} to carry a description, got {:?}",
                parameter["description"]
            );
            (name.clone(), required)
        })
        .collect();
    pairs.sort();
    pairs
}

/// The expected (name, required) pairs of a row, sorted by name.
fn expected_pairs(parameters: &[(&str, bool)]) -> Vec<(String, bool)> {
    let mut pairs: Vec<(String, bool)> = parameters
        .iter()
        .map(|(name, required)| ((*name).to_string(), *required))
        .collect();
    pairs.sort();
    pairs
}

/// The ARN fields of a catalogued entry, as a client parsing the ARN reads them.
///
/// Split at the first five colons only: the sixth field is the resource, and an action's id
/// carries colons of its own (`action/aws:<service>:<action>`).
fn arn_fields(case: &str, arn: &str) -> Vec<String> {
    let fields: Vec<String> = arn.splitn(6, ':').map(str::to_string).collect();
    assert_eq!(
        fields.len(),
        6,
        "case {case:?}: expected an ARN of six colon-separated fields, got {arn:?}"
    );
    fields
}

#[tokio::test]
async fn every_published_action_is_served_with_its_published_contract() {
    let router = emulator();
    let listed = list_actions(&router, FALLBACK_REGION).await;

    for row in action_rows() {
        let case = row.id;

        // The point read serves the published entry.
        let answer = get_action(&router, row.id, FALLBACK_REGION).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetAction for a published action, got {} \
             with {:?}",
            answer.status, answer.body
        );
        let action = &answer.body["action"];
        assert_eq!(
            action["id"].as_str(),
            Some(row.id),
            "case {case:?}: expected the reported id to be {:?}, got {:?}",
            row.id,
            action["id"]
        );
        if let Some(published) = row.description {
            assert_eq!(
                action["description"].as_str(),
                Some(published),
                "case {case:?}: expected the published description {published:?}, got {:?}",
                action["description"]
            );
        } else {
            assert!(
                action["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {case:?}: expected a non-empty description, got {:?}",
                action["description"]
            );
        }
        assert_eq!(
            parameter_pairs(case, action),
            expected_pairs(row.parameters),
            "case {case:?}: expected the published parameter contract {:?}, got {:?}",
            row.parameters,
            action["parameters"]
        );
        let targets = action["targets"]
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a targets map, got {action:?}"));
        assert_eq!(
            targets.len(),
            row.targets.len(),
            "case {case:?}: expected {} target(s), got {:?}",
            row.targets.len(),
            action["targets"]
        );
        for (key, resource_type) in row.targets {
            assert_eq!(
                targets[*key]["resourceType"].as_str(),
                Some(*resource_type),
                "case {case:?}: expected target {key:?} to name {resource_type:?}, got {:?}",
                action["targets"]
            );
        }

        // ... and the listing publishes the same entry, minus the parameter contract
        // `ActionSummary` does not model.
        let summary = listed
            .iter()
            .find(|entry| entry["id"].as_str() == Some(row.id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected ListActions to publish the action, got {:?}",
                    listed
                        .iter()
                        .filter_map(|entry| entry["id"].as_str())
                        .collect::<Vec<&str>>()
                )
            });
        for member in ["id", "arn", "description", "targets", "tags"] {
            assert_eq!(
                summary[member], action[member],
                "case {case:?}: expected the listed {member:?} to agree with the read one, \
                 got {:?} and {:?}",
                summary[member], action[member]
            );
        }
    }
}

#[tokio::test]
async fn every_published_resource_type_is_served_with_its_published_contract() {
    let router = emulator();
    let listed = list_resource_types(&router).await;

    for row in type_rows() {
        let case = row.id;
        let answer = get_resource_type(&router, row.id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 from GetTargetResourceType for a published type, \
             got {} with {:?}",
            answer.status, answer.body
        );
        let reported = &answer.body["targetResourceType"];
        assert_eq!(
            reported["resourceType"].as_str(),
            Some(row.id),
            "case {case:?}: expected the reported resourceType to be {:?}, got {:?}",
            row.id,
            reported["resourceType"]
        );
        if let Some(published) = row.description {
            assert_eq!(
                reported["description"].as_str(),
                Some(published),
                "case {case:?}: expected the published description {published:?}, got {:?}",
                reported["description"]
            );
        } else {
            assert!(
                reported["description"]
                    .as_str()
                    .is_some_and(|text| !text.is_empty()),
                "case {case:?}: expected a non-empty description, got {:?}",
                reported["description"]
            );
        }
        assert_eq!(
            parameter_pairs(case, reported),
            expected_pairs(row.parameters),
            "case {case:?}: expected the published parameter contract {:?}, got {:?}",
            row.parameters,
            reported["parameters"]
        );

        let summary = listed
            .iter()
            .find(|entry| entry["resourceType"].as_str() == Some(row.id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected ListTargetResourceTypes to publish the type, got \
                     {:?}",
                    listed
                        .iter()
                        .filter_map(|entry| entry["resourceType"].as_str())
                        .collect::<Vec<&str>>()
                )
            });
        for member in ["resourceType", "description"] {
            assert_eq!(
                summary[member], reported[member],
                "case {case:?}: expected the listed {member:?} to agree with the read one, \
                 got {:?} and {:?}",
                summary[member], reported[member]
            );
        }
    }
}

#[tokio::test]
async fn a_catalogued_action_arn_names_the_request_region_and_the_serving_account() {
    let router = emulator();
    // The rows are regions: three partitions' regions and a scope that offers none usable,
    // so the ARN is read off the request rather than off one region. Every action the
    // catalog publishes is checked in each of them, through BOTH surfaces that report one.
    for (signed, region) in [
        ("eu-west-2", "eu-west-2"),
        ("ap-southeast-3", "ap-southeast-3"),
        ("us-gov-east-1", "us-gov-east-1"),
        ("not a region", FALLBACK_REGION),
    ] {
        for summary in list_actions(&router, signed).await {
            let id = summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed action carries no id: {summary:?}"));
            let case = format!("{signed}/{id}");
            for (surface, arn) in [
                ("ListActions", summary["arn"].as_str().unwrap_or_default()),
                (
                    "GetAction",
                    get_action(&router, id, signed).await.body["action"]["arn"]
                        .as_str()
                        .unwrap_or_default(),
                ),
            ] {
                let fields = arn_fields(&case, arn);
                assert_eq!(
                    fields[..3].to_vec(),
                    vec!["arn".to_string(), "aws".to_string(), "fis".to_string()],
                    "case {case:?} via {surface}: expected an arn:aws:fis ARN, got {arn:?}"
                );
                assert_eq!(
                    fields[3], region,
                    "case {case:?} via {surface}: expected the ARN to name region {region:?}, \
                     got {arn:?}"
                );
                assert_eq!(
                    fields[4], ACCOUNT_ID,
                    "case {case:?} via {surface}: expected the ARN's account field to hold the \
                     serving account {ACCOUNT_ID:?}, got {arn:?}"
                );
                assert_eq!(
                    fields[5],
                    format!("action/{id}"),
                    "case {case:?} via {surface}: expected the ARN to name the action resource, \
                     got {arn:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_listing_and_a_read_of_either_catalog_agree_entry_for_entry() {
    let router = emulator();

    // Causes 4 and 6 of the issue read the entry a listing reports at an index, so the rows
    // here are indices into each list: its head, its second and third entries, and its
    // tail. Each catalog is enumerated in the order the service publishes it in, which is
    // what makes an index stable.
    let actions = list_actions(&router, FALLBACK_REGION).await;
    let action_ids: Vec<&str> = actions
        .iter()
        .map(|entry| {
            entry["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed action carries no id: {entry:?}"))
        })
        .collect();
    let mut sorted = action_ids.clone();
    sorted.sort_unstable();
    assert_eq!(
        action_ids, sorted,
        "case actions: expected the catalog to be published in id order, got {action_ids:?}"
    );
    assert_eq!(
        action_ids.iter().collect::<BTreeSet<&&str>>().len(),
        action_ids.len(),
        "case actions: expected every published action once, got {action_ids:?}"
    );

    for index in [0, 1, 2, action_ids.len() - 1] {
        let id = action_ids[index];
        let case = format!("actions[{index}]");
        let answer = get_action(&router, id, FALLBACK_REGION).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 reading the listed action {id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        for member in ["id", "arn", "description", "targets", "tags"] {
            assert_eq!(
                actions[index][member], answer.body["action"][member],
                "case {case:?}: expected the read {member:?} to be the listed one, got {:?} \
                 and {:?}",
                actions[index][member], answer.body["action"][member]
            );
        }
    }

    let types = list_resource_types(&router).await;
    let type_ids: Vec<&str> = types
        .iter()
        .map(|entry| {
            entry["resourceType"]
                .as_str()
                .unwrap_or_else(|| panic!("a listed resource type carries no id: {entry:?}"))
        })
        .collect();
    assert_eq!(
        type_ids.iter().collect::<BTreeSet<&&str>>().len(),
        type_ids.len(),
        "case targetResourceTypes: expected every published resource type once, got \
         {type_ids:?}"
    );
    // The ORDER the ids come in is the published one rather than ascending, and
    // `published_catalog_text.rs` asserts it index by index; what matters here is only that
    // an index is stable enough to read the entry back by.

    for index in [0, 1, 2, type_ids.len() - 1] {
        let id = type_ids[index];
        let case = format!("targetResourceTypes[{index}]");
        let answer = get_resource_type(&router, id).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200 reading the listed type {id:?}, got {} with {:?}",
            answer.status, answer.body
        );
        for member in ["resourceType", "description"] {
            assert_eq!(
                types[index][member], answer.body["targetResourceType"][member],
                "case {case:?}: expected the read {member:?} to be the listed one, got {:?} \
                 and {:?}",
                types[index][member], answer.body["targetResourceType"][member]
            );
        }
    }
}

#[tokio::test]
async fn every_resource_type_a_published_action_targets_is_published_too() {
    // The two catalogs come from the same reference, so a target an action offers is a type
    // a client can then read. Driven from the catalogs themselves, so this holds for every
    // published action and not only for the ones an issue named.
    let router = emulator();
    for summary in list_actions(&router, FALLBACK_REGION).await {
        let id = summary["id"]
            .as_str()
            .unwrap_or_else(|| panic!("a listed action carries no id: {summary:?}"));
        let targets = summary["targets"]
            .as_object()
            .unwrap_or_else(|| panic!("case {id:?}: expected a targets map, got {summary:?}"));
        for (key, target) in targets {
            let resource_type = target["resourceType"].as_str().unwrap_or_else(|| {
                panic!("case {id:?}: target {key:?} names no resourceType: {target:?}")
            });
            let case = format!("{id}/{key}");
            let answer = get_resource_type(&router, resource_type).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the targeted type {resource_type:?} to be published, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.body["targetResourceType"]["resourceType"].as_str(),
                Some(resource_type),
                "case {case:?}: expected the read to report {resource_type:?}, got {:?}",
                answer.body["targetResourceType"]
            );
        }
    }
}

#[tokio::test]
async fn an_action_id_the_catalog_does_not_publish_draws_the_one_fixed_refusal() {
    let router = emulator();
    // Each row is a different way of naming no action — a service FIS has no actions for, a
    // published id with its case changed, one truncated, one suffixed, an unqualified word
    // and a near miss of the entry this issue added — so the refusal is shown to be fixed
    // text rather than one built from the value.
    let unknown = [
        "aws:not-a-real-service:fake-action",
        "AWS:ARC:START-ZONAL-AUTOSHIFT",
        "aws:arc:start-zonal-autoshif",
        "aws:arc:start-zonal-autoshift:v2",
        "start-zonal-autoshift",
        "aws:arc:stop-zonal-autoshift",
    ];
    for id in unknown {
        let answer = get_action(&router, id, FALLBACK_REGION).await;
        assert_eq!(
            answer.status, 404,
            "case {id:?}: expected HTTP 404 for an id the catalog does not publish, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().ends_with("ResourceNotFoundException"),
            "case {id:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.message(),
            ACTION_NOT_FOUND,
            "case {id:?}: expected the fixed message {ACTION_NOT_FOUND:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains(id),
            "case {id:?}: the refusal must not echo the requested id, got {:?}",
            answer.message()
        );
    }
}
