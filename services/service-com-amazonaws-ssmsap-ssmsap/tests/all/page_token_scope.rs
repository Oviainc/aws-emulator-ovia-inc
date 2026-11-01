// SPDX-License-Identifier: CC0-1.0

//! What a continuation token of this service is BOUND to, driven over the wire.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint validation, the handler and
//! the response serialization all run exactly as they do in the emulator binary.
//!
//! RULE under test: a `NextToken` names a position in ONE listing taken over ONE scope —
//! the regional endpoint the listing was taken at, and the parent record or records the
//! request narrowed it to — so a later call to that listing over that scope resumes from
//! it, while the same listing over any OTHER scope refuses it as a token it never handed
//! out (the 400 `ValidationException` each of these operations models for a `NextToken`
//! it cannot resolve) rather than applying its key to items it names no position among.
//!
//! The rows span what the rule generalises over: which listing is paged (every paged
//! listing of this service that has more than one scope), what narrows it (a configuration
//! check run, a sub-check result, an operation record, an application, a component, the
//! regional endpoint), and how the two scopes of a row differ — in a narrowing both
//! requests carry, in an optional narrowing one of them leaves out, or in the endpoint
//! alone. Each row is driven in both directions, so neither scope is privileged.
//!
//! `tests/all/component_listing.rs` pins the other half of the same rule (a token of
//! another listing KIND is refused too), and the catalogue listing — whose items are the
//! service's own and which no Region and no record narrows — is the row here that must
//! still resume at any endpoint.
//!
//! One thing does NOT go through the wire: the sub-check results of a run are PLACED in
//! the store, for the reason `tests/all/sub_check_results.rs` gives (a run produces only
//! the results its check's rules decide), and so that the two runs' result ids are known
//! to sort apart — a token one run issued names a key ABOVE everything the other run
//! holds, which is what a listing that accepted it would silently report nothing for.

use serde_json::{json, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    COMPONENT_TYPE_VALUES, CONFIGURATION_CHECK_TYPE_VALUES,
};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply, REGION};
use crate::configuration_checks::{started, string_field};
use crate::list_databases::{credentials, listed as listed_databases};
use crate::sub_check_results::{place_results, sub_check_results};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// A second regional endpoint: every record of this service belongs to the Region the
/// call that filed it was made in, so the endpoint a listing was taken at is part of the
/// scope its token names a position in.
const ELSEWHERE: &str = "eu-west-1";

/// A third endpoint, which nothing is registered at: the catalogue row resumes there too,
/// because the catalogue is the service's own and no Region narrows it.
const UNUSED_REGION: &str = "ap-southeast-2";

/// The two applications registered at [`REGION`].
const APPLICATION_A: &str = "hdb-token-a";
const APPLICATION_B: &str = "hdb-token-b";

/// The two applications registered at [`ELSEWHERE`]. Their ids sort BELOW the pair at
/// [`REGION`], so a token a Region-wide listing handed out at [`REGION`] names a key above
/// everything the other endpoint holds — the case where a listing that accepted a foreign
/// token reports an empty page and no token at all.
const APPLICATION_C: &str = "abap-token-c";
const APPLICATION_D: &str = "abap-token-d";

/// The paths the paged listings are bound to (`smithy_query(query="digest")`).
const SUB_CHECK_RESULTS: &str = "/list-sub-check-results";
const RULE_RESULTS: &str = "/list-sub-check-rule-results";
const OPERATION_EVENTS: &str = "/list-operation-events";
const OPERATIONS: &str = "/list-operations";
const CHECK_OPERATIONS: &str = "/list-configuration-check-operations";
const COMPONENTS: &str = "/list-components";
const DATABASES: &str = "/list-databases";
const APPLICATIONS: &str = "/list-applications";
const CHECK_DEFINITIONS: &str = "/list-configuration-check-definitions";

/// How many pages a walk follows before it gives up: a listing that handed out a token
/// that does not advance fails its case rather than looping forever.
const PAGE_LIMIT: usize = 64;

/// One scope a listing can be taken over: the endpoint the call is signed for and the
/// members that narrow what it reports.
#[derive(Debug)]
struct Scope {
    /// How an assertion names this scope.
    label: String,
    region: &'static str,
    /// The request body, without `MaxResults` and `NextToken`.
    body: Value,
}

/// One listing and two scopes of it, the row the rule is driven on.
#[derive(Debug)]
struct Case {
    label: String,
    path: &'static str,
    /// The response member this listing reports its items in.
    items: &'static str,
    scopes: [Scope; 2],
}

/// The items a listing reported, in the order it reported them.
fn items(reply: &Reply, member: &str) -> Vec<Value> {
    reply
        .member(member)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The `NextToken` a reply handed out, absent when it reported the last page.
fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// One request to a listing at one scope, carrying `MaxResults` and `NextToken` only when
/// given.
async fn request(
    emulator: &Emulator,
    case: &Case,
    scope: &Scope,
    max_results: Option<i32>,
    token: Option<&str>,
) -> Reply {
    let mut body = scope
        .body
        .as_object()
        .cloned()
        .expect("a listing request body is a JSON object");
    if let Some(max_results) = max_results {
        body.insert("MaxResults".to_string(), json!(max_results));
    }
    if let Some(token) = token {
        body.insert("NextToken".to_string(), json!(token));
    }
    emulator
        .send_in_region(scope.region, case.path, Value::Object(body))
        .await
}

/// Follow one scope's pages to exhaustion at `max_results` items per page, and return
/// every item reported, in the order the pages reported them.
async fn walk(
    emulator: &Emulator,
    case: &Case,
    scope: &Scope,
    max_results: Option<i32>,
    name: &str,
) -> Vec<Value> {
    let mut seen: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..PAGE_LIMIT {
        let reply = request(emulator, case, scope, max_results, token.as_deref()).await;
        assert_eq!(
            reply.status, 200,
            "case {name:?} page {page} of {:?}: expected 200, got {reply:?}",
            scope.label
        );
        if let Some(limit) = max_results {
            let reported = items(&reply, case.items).len();
            assert!(
                reported <= limit as usize,
                "case {name:?} page {page} of {:?}: expected at most {limit} items, got \
                 {reported}",
                scope.label
            );
        }
        seen.extend(items(&reply, case.items));
        token = next_token(&reply);
        if token.is_none() {
            return seen;
        }
    }
    panic!(
        "case {name:?}: {:?} handed out tokens for more than {PAGE_LIMIT} pages",
        scope.label
    );
}

/// A `RegisterApplication` body for a HANA application declaring two components beside the
/// SAP HANA component of its own System ID, and credentials naming two databases — so
/// every listing scoped to ONE application holds more than one item and therefore hands
/// out a token at `MaxResults` 1.
///
/// The component types come from the model's own `ComponentType` catalogue, and each
/// declaration gets its own `Ec2InstanceId` (inside the modelled `^i-[\w\d]{17}$`).
fn registration(id: &str) -> Value {
    let mut body = valid_registration(id, "HANA");
    let object = body
        .as_object_mut()
        .expect("a registration body is a JSON object");
    object.insert(
        "ComponentsInfo".to_string(),
        Value::Array(
            [("AAA", 0usize), ("ZZ9", 1usize)]
                .into_iter()
                .map(|(sid, index)| {
                    json!({
                        "ComponentType": COMPONENT_TYPE_VALUES[index % COMPONENT_TYPE_VALUES.len()],
                        "Sid": sid,
                        "Ec2InstanceId": format!("i-{index:017}"),
                    })
                })
                .collect(),
        ),
    );
    // Two distinct `DatabaseName`s, so the databases listing of one application holds two
    // databases.
    object.insert("Credentials".to_string(), credentials(&["SYSTEMDB", "TNT1"]));
    body
}

/// Register one application at `region` and return the `OperationId` the registration
/// reported.
async fn register(emulator: &Emulator, region: &'static str, id: &str) -> String {
    let reply = emulator
        .send_in_region(region, "/register-application", registration(id))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {region:?}/{id:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Start the first two checks of the model's catalogue against `application_id` and return
/// the `OperationId` of each run, so the check-operations listing of one application holds
/// more than one record.
async fn start_two_checks(emulator: &Emulator, application_id: &str) -> Vec<String> {
    assert!(
        CONFIGURATION_CHECK_TYPE_VALUES.len() >= 2,
        "the rows need two distinct checks, the model's catalogue holds {}",
        CONFIGURATION_CHECK_TYPE_VALUES.len()
    );
    let reply = emulator
        .start_checks(json!({
            "ApplicationId": application_id,
            "ConfigurationCheckIds": [
                CONFIGURATION_CHECK_TYPE_VALUES[0],
                CONFIGURATION_CHECK_TYPE_VALUES[1],
            ],
        }))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application_id:?}: expected 200 from StartConfigurationChecks, got {reply:?}"
    );
    let runs = started(&reply);
    assert_eq!(
        runs.len(),
        2,
        "case {application_id:?}: expected two started runs, got {runs:?}"
    );
    runs.iter().map(|run| string_field(run, "Id")).collect()
}

/// The `ComponentId` the databases of `application_id` belong to, read off the listing
/// rather than derived here, so the row that narrows by component narrows by a value the
/// service itself reports.
async fn database_component(emulator: &Emulator, application_id: &str) -> String {
    let reply = emulator
        .send_in_region(REGION, DATABASES, json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application_id:?}: expected 200 from ListDatabases, got {reply:?}"
    );
    let reported = listed_databases(&reply);
    reported
        .first()
        .and_then(|database| database.component_id.clone())
        .unwrap_or_else(|| {
            panic!("case {application_id:?}: expected a database carrying a ComponentId, got {reported:?}")
        })
}

/// One scope, spelled once.
fn scope(label: &str, region: &'static str, body: Value) -> Scope {
    Scope {
        label: label.to_string(),
        region,
        body,
    }
}

/// The emulator the rows are driven against, and the rows themselves.
///
/// Two applications at [`REGION`] and two at [`ELSEWHERE`], each with its components and
/// databases; a second operation record per application at [`REGION`] (a start, beside the
/// registration); two configuration check runs per application there; and placed sub-check
/// results on one run of each application, each run's results in their own id family.
async fn fixture() -> (Emulator, Vec<Case>) {
    let emulator = Emulator::new();

    let registration_a = register(&emulator, REGION, APPLICATION_A).await;
    let registration_b = register(&emulator, REGION, APPLICATION_B).await;
    register(&emulator, ELSEWHERE, APPLICATION_C).await;
    register(&emulator, ELSEWHERE, APPLICATION_D).await;
    settle().await;

    // A second operation record per application of this Region, so its operations listing
    // holds more than one item.
    for application_id in [APPLICATION_A, APPLICATION_B] {
        let reply = emulator
            .send_in_region(
                REGION,
                "/start-application",
                json!({"ApplicationId": application_id}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {application_id:?}: expected 200 from StartApplication, got {reply:?}"
        );
    }
    settle().await;

    let runs_a = start_two_checks(&emulator, APPLICATION_A).await;
    let runs_b = start_two_checks(&emulator, APPLICATION_B).await;

    // Each run's placed results sit in their own id family, so one run's keys all sort
    // above the other's.
    let stored_a = sub_check_results(0xA0, 3, 2);
    place_results(&emulator, &runs_a[0], stored_a.clone()).await;
    let stored_b = sub_check_results(0xB0, 3, 2);
    place_results(&emulator, &runs_b[0], stored_b.clone()).await;

    let component = database_component(&emulator, APPLICATION_A).await;

    let cases = vec![
        Case {
            label: "ListSubCheckResults over two configuration check runs".to_string(),
            path: SUB_CHECK_RESULTS,
            items: "SubCheckResults",
            scopes: [
                scope(
                    "the run of the first application",
                    REGION,
                    json!({"OperationId": runs_a[0]}),
                ),
                scope(
                    "the run of the second application",
                    REGION,
                    json!({"OperationId": runs_b[0]}),
                ),
            ],
        },
        Case {
            label: "ListSubCheckRuleResults over two sub-check results".to_string(),
            path: RULE_RESULTS,
            items: "RuleResults",
            scopes: [
                scope(
                    "a sub-check result of the first run",
                    REGION,
                    json!({"SubCheckResultId": stored_a[0].id}),
                ),
                scope(
                    "a sub-check result of the second run",
                    REGION,
                    json!({"SubCheckResultId": stored_b[0].id}),
                ),
            ],
        },
        Case {
            label: "ListOperationEvents over two operation records".to_string(),
            path: OPERATION_EVENTS,
            items: "OperationEvents",
            scopes: [
                scope(
                    "the first application's registration",
                    REGION,
                    json!({"OperationId": registration_a}),
                ),
                scope(
                    "the second application's registration",
                    REGION,
                    json!({"OperationId": registration_b}),
                ),
            ],
        },
        Case {
            label: "ListOperations over two applications".to_string(),
            path: OPERATIONS,
            items: "Operations",
            scopes: [
                scope(
                    "the first application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope(
                    "the second application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_B}),
                ),
            ],
        },
        Case {
            label: "ListConfigurationCheckOperations over two applications".to_string(),
            path: CHECK_OPERATIONS,
            items: "ConfigurationCheckOperations",
            scopes: [
                scope(
                    "the first application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope(
                    "the second application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_B}),
                ),
            ],
        },
        Case {
            label: "ListComponents over two applications".to_string(),
            path: COMPONENTS,
            items: "Components",
            scopes: [
                scope(
                    "the first application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope(
                    "the second application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_B}),
                ),
            ],
        },
        Case {
            label: "ListComponents narrowed to one application and over the whole Region"
                .to_string(),
            path: COMPONENTS,
            items: "Components",
            scopes: [
                scope(
                    "one application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope("every application of the Region", REGION, json!({})),
            ],
        },
        Case {
            label: "ListComponents over the whole Region at two endpoints".to_string(),
            path: COMPONENTS,
            items: "Components",
            scopes: [
                scope("one endpoint", REGION, json!({})),
                scope("another endpoint", ELSEWHERE, json!({})),
            ],
        },
        Case {
            label: "ListDatabases over two applications".to_string(),
            path: DATABASES,
            items: "Databases",
            scopes: [
                scope(
                    "the first application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope(
                    "the second application",
                    REGION,
                    json!({"ApplicationId": APPLICATION_B}),
                ),
            ],
        },
        Case {
            label: "ListDatabases of one application, narrowed by its component and not"
                .to_string(),
            path: DATABASES,
            items: "Databases",
            scopes: [
                scope(
                    "the application alone",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A}),
                ),
                scope(
                    "the application and its component",
                    REGION,
                    json!({"ApplicationId": APPLICATION_A, "ComponentId": component}),
                ),
            ],
        },
        Case {
            label: "ListDatabases over the whole Region at two endpoints".to_string(),
            path: DATABASES,
            items: "Databases",
            scopes: [
                scope("one endpoint", REGION, json!({})),
                scope("another endpoint", ELSEWHERE, json!({})),
            ],
        },
        Case {
            label: "ListApplications at two endpoints".to_string(),
            path: APPLICATIONS,
            items: "Applications",
            scopes: [
                scope("one endpoint", REGION, json!({})),
                scope("another endpoint", ELSEWHERE, json!({})),
            ],
        },
    ];

    (emulator, cases)
}

/// RULE: a token names a position in the listing AND the scope it was handed out for —
/// the scope that issued it resumes from it, and the same listing over any other scope
/// refuses it as a token it never issued, rather than applying its key to items it names
/// no position among.
#[tokio::test]
async fn a_token_resumes_only_the_scope_it_was_issued_for() {
    let (emulator, cases) = fixture().await;

    for case in &cases {
        for (issuer, other) in [(0usize, 1usize), (1, 0)] {
            let issuer = &case.scopes[issuer];
            let other = &case.scopes[other];
            let name = format!(
                "{}: a token of {:?} presented to {:?}",
                case.label, issuer.label, other.label
            );

            // What the other scope holds, before any foreign token is presented to it.
            let whole = walk(&emulator, case, other, None, &name).await;
            assert!(
                whole.len() >= 2,
                "case {name:?}: the row needs {:?} to hold at least two items, got {}",
                other.label,
                whole.len()
            );

            // A token the issuing scope hands out, one item in.
            let first = request(&emulator, case, issuer, Some(1), None).await;
            assert_eq!(
                first.status, 200,
                "case {name:?}: expected 200 from {:?}, got {first:?}",
                issuer.label
            );
            let token = next_token(&first).unwrap_or_else(|| {
                panic!(
                    "case {name:?}: expected {:?} to hand out a token, got {first:?}",
                    issuer.label
                )
            });

            // The scope that issued it resumes from it.
            let resumed = request(&emulator, case, issuer, Some(1), Some(&token)).await;
            assert_eq!(
                resumed.status, 200,
                "case {name:?}: a scope resumes from its own token, got {resumed:?}"
            );
            let resumed_items = items(&resumed, case.items);
            assert_eq!(
                resumed_items.len(),
                1,
                "case {name:?}: expected the one item after the first, got {resumed_items:?}"
            );

            // No other scope does.
            let refused = request(&emulator, case, other, None, Some(&token)).await;
            assert_eq!(
                refused.status, 400,
                "case {name:?}: expected 400, got {refused:?}"
            );
            assert_eq!(
                refused.code(),
                VALIDATION_EXCEPTION,
                "case {name:?}: expected {VALIDATION_EXCEPTION}, got {refused:?}"
            );
            let expected = format!("NextToken {token} is not a valid pagination token");
            let stated = message(&refused);
            assert_eq!(
                stated, expected,
                "case {name:?}: expected {expected:?}, got {stated:?}"
            );

            // And the refusal left that scope's own paging reporting every item it holds,
            // exactly once and in one order, whatever page size it is walked at.
            for max_results in [Some(1), Some(2), None] {
                let walked = walk(&emulator, case, other, max_results, &name).await;
                assert_eq!(
                    walked, whole,
                    "case {name:?} at MaxResults {max_results:?}: expected every item of \
                     {:?} exactly once, got {walked:?}",
                    other.label
                );
            }
        }
    }
}

/// RULE, the other side of the same one: a listing that NOTHING narrows has one scope, so
/// its token stays resumable wherever it is presented. The configuration check definitions
/// are the service's own catalogue, which no Region and no record narrows.
#[tokio::test]
async fn the_catalogue_listings_token_resumes_at_any_endpoint() {
    let emulator = Emulator::new();

    let whole = emulator
        .send_in_region(REGION, CHECK_DEFINITIONS, json!({}))
        .await;
    assert_eq!(
        whole.status, 200,
        "expected 200 from ListConfigurationCheckDefinitions, got {whole:?}"
    );
    let catalogue = items(&whole, "ConfigurationChecks");
    assert!(
        catalogue.len() >= 2,
        "the case needs a catalogue of at least two definitions, got {catalogue:?}"
    );

    let first = emulator
        .send_in_region(REGION, CHECK_DEFINITIONS, json!({"MaxResults": 1}))
        .await;
    assert_eq!(first.status, 200, "expected 200, got {first:?}");
    let token = next_token(&first)
        .unwrap_or_else(|| panic!("expected a token for a catalogue of {} items, got {first:?}", catalogue.len()));

    for region in [REGION, ELSEWHERE, UNUSED_REGION] {
        let resumed = emulator
            .send_in_region(region, CHECK_DEFINITIONS, json!({"NextToken": token}))
            .await;
        assert_eq!(
            resumed.status, 200,
            "case {region:?}: expected 200, got {resumed:?}"
        );
        let reported = items(&resumed, "ConfigurationChecks");
        assert_eq!(
            reported,
            catalogue[1..].to_vec(),
            "case {region:?}: expected every definition after the first, got {reported:?}"
        );
    }
}
