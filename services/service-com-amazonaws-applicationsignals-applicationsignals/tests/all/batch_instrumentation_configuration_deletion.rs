// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of `BatchDeleteInstrumentationConfigurations` over HTTP, with the
//! `GetInstrumentationConfiguration` and `ListInstrumentationConfigurations` readers that
//! answer what is left.
//!
//! The rule under test: one call removes a SET of dynamic instrumentation configurations and
//! reports each removal. The set is the one the modelled `BatchDeleteDeletionTarget` names —
//! by SCOPE every configuration of a service, environment and instrumentation type, each
//! reported by the signal type and location hash that identified it there; by ARN LIST the
//! configurations of the named instrumentation type those ARNs resolve to, each reported by
//! its ARN, with every ARN that resolves to nothing failing that ITEM (the modelled
//! `ResourceNotFoundException` code) rather than the request. `DeletedCount` is how many
//! configurations the call removed, which is the length of `SuccessfulDeletions`, and
//! nothing outside the selected set changes.
//!
//! WHICH configuration a listed ARN resolves to is the identity its RESOURCE field spells:
//! the partition and region an ARN carries narrow nothing, so an ARN of this service and
//! account names the one stored configuration in every spelling of them.
//!
//! Every assertion is made over a TABLE of selections rather than on one request: the rows
//! differ in the selection METHOD, in how many configurations the selection names (none, one,
//! several), in whether the named resources exist, in the instrumentation type the target
//! carries, and in the scope fields the listed ARNs are written with. The request helpers
//! drive the same `restJson1` surface `slo_lifecycle` mounts.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{arn_uri_segment, call, router, ACCOUNT, REGION};
// The sentence a per-item failure's `Message` states, owned by the suite that holds this
// service's refusal prose.
use crate::refusal_prose::CONFIGURATION_GONE;
// The scope spellings an ARN of this service may be written with, and the rewriter that
// produces them, shared with the suite that asserts the same rule for the tagging operations.
use crate::tagged_arn_identity::{respelled, spellings};

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` ->
/// `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values the model declares
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The `BatchDeleteErrorCode` a per-item failure carries for an ARN that names no stored
/// configuration (`smithy_query(query="shape",shape="BatchDeleteErrorCode")` ->
/// `["ResourceNotFoundException", "AccessDeniedException", "InternalServiceException"]`).
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /get-instrumentation-configuration`.
async fn get(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/get-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /list-instrumentation-configurations` — the reader that reports a whole scope's set.
async fn list(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/list-instrumentation-configurations",
        Some(body),
    )
    .await
}

/// `POST /batch-delete-instrumentation-configurations`.
async fn batch_delete(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/batch-delete-instrumentation-configurations",
        Some(body),
    )
    .await
}

/// `GET /tags?ResourceArn=<arn>` — the reader that reports a resource's tags.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tags?ResourceArn={}", arn_uri_segment(arn)),
        None,
    )
    .await
}

/// The selectors every instrumentation operation takes beside the location, which are also
/// the members of a `BatchDeleteScope`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Scope {
    instrumentation_type: &'static str,
    service: &'static str,
    environment: &'static str,
}

impl Scope {
    /// A `DeletionTarget` selecting this whole scope.
    fn deletion_target(&self) -> Value {
        json!({
            "DeletionTarget": {
                "Scope": {
                    "Service": self.service,
                    "Environment": self.environment,
                    "InstrumentationType": self.instrumentation_type,
                }
            }
        })
    }

    /// A `ListInstrumentationConfigurations` request body for this scope.
    fn list_body(&self) -> Value {
        json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
        })
    }
}

/// A `DeletionTarget` selecting `arns` within `instrumentation_type`.
fn arn_list_target(instrumentation_type: &str, arns: &[String]) -> Value {
    json!({
        "DeletionTarget": {
            "ResourceArns": {
                "ResourceArns": arns,
                "InstrumentationType": instrumentation_type,
            }
        }
    })
}

/// One configuration the fixture stores: a scope, a code location within it, and the tags
/// attached to it.
struct Definition {
    label: &'static str,
    scope: Scope,
    language: &'static str,
    /// The `ClassName` the location names, which `CodeLocation` documents as "required for
    /// Java and optional for Python module-level functions" — so a Java row carries one and
    /// a row of another language may leave it out.
    class_name: Option<&'static str>,
    file_path: &'static str,
    line_number: i64,
    tags: Option<Value>,
}

impl Definition {
    /// The `CreateInstrumentationConfiguration` request body for this row.
    fn create_body(&self) -> Value {
        let mut body = json!({
            "InstrumentationType": self.scope.instrumentation_type,
            "Service": self.scope.service,
            "Environment": self.scope.environment,
            "SignalType": SNAPSHOT,
            "Location": {
                "CodeLocation": {
                    "Language": self.language,
                    "FilePath": self.file_path,
                    "LineNumber": self.line_number,
                }
            },
            "CaptureConfiguration": {
                "CodeCapture": { "CaptureLimits": { "MaxHits": 5 } }
            },
        });
        if let Some(class_name) = self.class_name {
            body["Location"]["CodeLocation"]["ClassName"] = json!(class_name);
        }
        if let Some(tags) = self.tags.as_ref() {
            body["Tags"] = tags.clone();
        }
        body
    }
}

/// A configuration the fixture created, with the identity the create reported for it.
struct Stored {
    definition: Definition,
    arn: String,
    location_hash: String,
}

impl Stored {
    /// A `GetInstrumentationConfiguration` body addressing this record by its location hash.
    fn get_body(&self) -> Value {
        json!({
            "InstrumentationType": self.definition.scope.instrumentation_type,
            "Service": self.definition.scope.service,
            "Environment": self.definition.scope.environment,
            "SignalType": SNAPSHOT,
            "LocationIdentifier": { "LocationHash": self.location_hash },
        })
    }
}

/// The scopes the fixture spreads its configurations over.
///
/// Two of them share a service and environment and differ only in the instrumentation type —
/// the selector that "determines which backing store to query" — so a selection of one type
/// must leave the other's configurations in place. The shortest names the modelled
/// `length 1..255` admits and a name carrying `:` and `/` are both represented.
const SCOPE_A: Scope = Scope {
    instrumentation_type: BREAKPOINT,
    service: "checkout",
    environment: "eks:cluster-prod/namespace",
};
const SCOPE_B: Scope = Scope {
    instrumentation_type: PROBE,
    service: "checkout",
    environment: "eks:cluster-prod/namespace",
};
const SCOPE_C: Scope = Scope {
    instrumentation_type: BREAKPOINT,
    service: "inventory",
    environment: "ec2:production",
};
const SCOPE_D: Scope = Scope {
    instrumentation_type: PROBE,
    service: "a",
    environment: "e",
};
/// A scope no create ever wrote to, so it holds nothing.
const SCOPE_EMPTY: Scope = Scope {
    instrumentation_type: BREAKPOINT,
    service: "unvisited",
    environment: "nowhere",
};

/// The configurations every test starts from: eight records over four scopes, so a selection
/// names none, one or several of them and there is always something outside it to check.
fn definitions() -> Vec<Definition> {
    vec![
        Definition {
            label: "A0 breakpoint, tagged",
            scope: SCOPE_A,
            language: "Java",
            class_name: Some("com.example.checkout.CartController"),
            file_path: "src/main/java/com/example/checkout/CartController.java",
            line_number: 142,
            tags: Some(json!([
                { "Key": "team", "Value": "payments" },
                { "Key": "env", "Value": "prod" }
            ])),
        },
        Definition {
            label: "A1 breakpoint, same file one line further down",
            scope: SCOPE_A,
            language: "Java",
            class_name: Some("com.example.checkout.CartController"),
            file_path: "src/main/java/com/example/checkout/CartController.java",
            line_number: 143,
            tags: None,
        },
        Definition {
            label: "A2 breakpoint, another file",
            scope: SCOPE_A,
            language: "Java",
            class_name: Some("com.example.checkout.Totals"),
            file_path: "src/main/java/com/example/checkout/Totals.java",
            line_number: 7,
            tags: Some(json!([{ "Key": "only", "Value": "one" }])),
        },
        Definition {
            label: "B0 probe in the same service and environment as A0",
            scope: SCOPE_B,
            language: "Java",
            class_name: Some("com.example.checkout.Checkout"),
            file_path: "src/main/java/com/example/checkout/Checkout.java",
            line_number: 61,
            tags: Some(json!([{ "Key": "team", "Value": "payments" }])),
        },
        Definition {
            label: "B1 probe, another file",
            scope: SCOPE_B,
            language: "Javascript",
            class_name: Some("Cart"),
            file_path: "web/src/cart.js",
            line_number: 1,
            tags: None,
        },
        Definition {
            label: "C0 breakpoint in another service",
            scope: SCOPE_C,
            language: "Python",
            class_name: None,
            file_path: "app/inventory/stock.py",
            line_number: 12,
            tags: None,
        },
        Definition {
            label: "C1 breakpoint in another service, another file",
            scope: SCOPE_C,
            language: "Python",
            class_name: Some("Reservations"),
            file_path: "app/inventory/reserve.py",
            line_number: 30,
            tags: Some(json!([{ "Key": "team", "Value": "stock" }])),
        },
        Definition {
            label: "D0 probe, the shortest service and environment names",
            scope: SCOPE_D,
            language: "Python",
            class_name: None,
            file_path: "main.py",
            line_number: 1,
            tags: None,
        },
    ]
}

/// Create every definition and answer the stored records, in definition order.
///
/// The ARN each create reports is checked against the format the modelled
/// `InstrumentationConfigurationArn` documents, so the ARNs the tests then delete by are the
/// ones a client reads off a create rather than strings this test invented.
async fn store_all(router: &MultiServiceRouter) -> Vec<Stored> {
    let mut stored = Vec::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    for definition in definitions() {
        let label = definition.label;
        let (status, created) = create(router, definition.create_body()).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the setup create to succeed, got {status} with {created}"
        );
        let location_hash = created["LocationHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let arn = created["ARN"].as_str().unwrap_or_default().to_string();
        let expected_arn = format!(
            "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/{}/{}/{SNAPSHOT}/{location_hash}",
            definition.scope.service, definition.scope.environment
        );
        assert_eq!(
            arn, expected_arn,
            "case {label:?}: expected the create to mint the documented ARN, got {arn:?}"
        );
        // The documented ARN format carries no instrumentation type, so the fixture keeps
        // every row at its own service, environment or location: an ARN here names exactly
        // one stored configuration.
        assert!(
            minted.insert(arn.clone()),
            "case {label:?}: expected the fixture rows to mint distinct ARNs, got {arn:?} twice"
        );
        stored.push(Stored {
            definition,
            arn,
            location_hash,
        });
    }
    stored
}

/// An ARN in the documented format whose location hash no create ever produced, so it names
/// no stored configuration. Longer than the modelled member minimum of 20 characters.
fn never_created_arn() -> String {
    format!(
        "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/checkout/eks:cluster-prod/namespace/{SNAPSHOT}/0123456789abcdef"
    )
}

/// The `(SignalType, LocationHash)` pairs a `SuccessfulDeletions` list reports, and an
/// assertion that each item is in the shape a scope deletion reports: both members present,
/// and no `ResourceArn` (the ARN form's member).
fn scope_deletions(case: &str, answer: &Value) -> BTreeSet<(String, String)> {
    let items = answer["SuccessfulDeletions"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut pairs = BTreeSet::new();
    for item in &items {
        assert!(
            item["ResourceArn"].is_null(),
            "case {case:?}: expected a scope deletion to report no ResourceArn, got {item}"
        );
        let signal_type = item["SignalType"].as_str().unwrap_or_default().to_string();
        let location_hash = item["LocationHash"].as_str().unwrap_or_default().to_string();
        assert!(
            !signal_type.is_empty() && !location_hash.is_empty(),
            "case {case:?}: expected a scope deletion to report SignalType and LocationHash, got {item}"
        );
        assert!(
            pairs.insert((signal_type, location_hash)),
            "case {case:?}: expected each deletion reported once, got {item} twice"
        );
    }
    pairs
}

/// The `ResourceArn`s a `SuccessfulDeletions` list reports, in the order it reports them, with
/// an assertion that each item is in the shape an ARN-list deletion reports.
fn arn_deletions(case: &str, answer: &Value) -> Vec<String> {
    answer["SuccessfulDeletions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let arn = item["ResourceArn"].as_str().unwrap_or_default().to_string();
            assert!(
                !arn.is_empty(),
                "case {case:?}: expected an ARN-list deletion to report its ResourceArn, got {item}"
            );
            arn
        })
        .collect()
}

/// Assert `answer` carries `DeletedCount` equal to the number of successful deletions it
/// reports, which is the count the output documents for either selection method.
fn assert_deleted_count(case: &str, answer: &Value, expected: usize) {
    let reported = answer["SuccessfulDeletions"]
        .as_array()
        .map(|items| items.len())
        .unwrap_or_default();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {expected} successful deletions, got {answer}"
    );
    assert_eq!(
        answer["DeletedCount"].as_i64(),
        Some(expected as i64),
        "case {case:?}: expected DeletedCount {expected}, got {answer}"
    );
}

/// The ARNs a `ListInstrumentationConfigurations` page reports.
fn listed_arns(page: &Value) -> Vec<String> {
    page["LatestConfigurations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| item["ARN"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Assert a reader answers the stored record (`survivor`) or refuses it as gone (`!survivor`).
async fn assert_readable(router: &MultiServiceRouter, case: &str, record: &Stored, present: bool) {
    let (status, answer) = get(router, record.get_body()).await;
    let label = record.definition.label;
    if present {
        assert_eq!(
            status, 200,
            "case {case:?}: expected {label:?} to survive the delete, got {status} with {answer}"
        );
        assert_eq!(
            answer["Configuration"]["ARN"].as_str(),
            Some(record.arn.as_str()),
            "case {case:?}: expected {label:?} to read back at its own ARN, got {answer}"
        );
    } else {
        assert_eq!(
            status, 404,
            "case {case:?}: expected {label:?} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException for {label:?}, got {answer}"
        );
    }
}

/// A scope selection deletes every configuration of that scope and nothing else: it reports
/// each removal by its signal type and location hash, counts them, leaves the scope's list
/// empty and every other scope untouched, takes the records' tags with them, and reports zero
/// for a scope that holds nothing — so repeating the call changes nothing further.
#[tokio::test]
async fn a_scope_selection_deletes_exactly_that_scopes_configurations() {
    let router = router();
    let stored = store_all(&router).await;

    // The rows differ in how many configurations the scope holds (none, one, two, three) and
    // in whether an earlier row already emptied it.
    let rows: [(&str, Scope); 6] = [
        ("a scope that never held a configuration", SCOPE_EMPTY),
        ("a scope holding three configurations", SCOPE_A),
        ("that same scope again, now empty", SCOPE_A),
        ("the other type of the same service and environment", SCOPE_B),
        ("a scope holding two configurations", SCOPE_C),
        ("a scope holding one", SCOPE_D),
    ];
    let mut remaining: Vec<&Stored> = stored.iter().collect();
    let mut gone: Vec<&Stored> = Vec::new();

    for (case, scope) in rows {
        let (selected, left): (Vec<&Stored>, Vec<&Stored>) = remaining
            .iter()
            .copied()
            .partition(|record| record.definition.scope == scope);
        // Two rows deliberately select nothing: a scope nothing was ever created in, and one
        // an earlier row emptied.
        let expected: BTreeSet<(String, String)> = selected
            .iter()
            .map(|record| (SNAPSHOT.to_string(), record.location_hash.clone()))
            .collect();

        let (status, answer) = batch_delete(&router, scope.deletion_target()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the batch delete, got {status} with {answer}"
        );
        assert!(
            answer["Errors"]
                .as_array()
                .is_some_and(|errors| errors.is_empty()),
            "case {case:?}: expected a scope selection to report no per-item errors, got {answer}"
        );
        assert_deleted_count(case, &answer, selected.len());
        assert_eq!(
            scope_deletions(case, &answer),
            expected,
            "case {case:?}: expected the scope's own configurations reported as deleted, got {answer}"
        );

        let (status, page) = list(&router, scope.list_body()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the list, got {status} with {page}"
        );
        assert_eq!(
            listed_arns(&page),
            Vec::<String>::new(),
            "case {case:?}: expected the scope to hold nothing after the delete, got {page}"
        );

        gone.extend(selected);
        remaining = left;

        for record in &gone {
            assert_readable(&router, case, record, false).await;
            if record.definition.tags.is_some() {
                let (status, tags) = list_tags(&router, &record.arn).await;
                assert_eq!(
                    status, 404,
                    "case {case:?}: expected the tags of {:?} to go with the record, got {status} with {tags}",
                    record.definition.label
                );
            }
        }
        for record in &remaining {
            assert_readable(&router, case, record, true).await;
        }
    }

    assert!(
        remaining.is_empty(),
        "expected the table to select every stored configuration, {} left",
        remaining.len()
    );
}

/// An ARN-list selection deletes the configurations its ARNs resolve to within the
/// instrumentation type it names, reports each by its ARN in request order, counts them, and
/// fails only the ITEMS whose ARN resolves to nothing — one never created, one already
/// deleted earlier in the same list, or one of the other instrumentation type — leaving every
/// other configuration of the store readable.
#[tokio::test]
async fn an_arn_list_selection_deletes_what_resolves_and_reports_the_rest_per_item() {
    let router = router();
    let stored = store_all(&router).await;
    let missing = never_created_arn();

    /// One row: the instrumentation type the target names, the ARNs it lists (by index into
    /// the fixture, or the never-created ARN), and which of them are expected to resolve.
    struct Row {
        case: &'static str,
        instrumentation_type: &'static str,
        /// `Some(index)` is the fixture record at that index; `None` is the never-created ARN.
        arns: Vec<Option<usize>>,
        /// The indexes this row is expected to delete, in the order they are listed.
        deleted: Vec<usize>,
    }

    // The rows differ in the number of ARNs listed (one, two, three), in whether they span
    // one scope or several, in whether an ARN resolves at all, and in the type the target
    // carries. Indexes follow `definitions()`: 0..3 scope A, 3..5 scope B, 5..7 scope C, 7
    // scope D.
    let rows = vec![
        Row {
            case: "one ARN of one scope",
            instrumentation_type: BREAKPOINT,
            arns: vec![Some(0)],
            deleted: vec![0],
        },
        Row {
            case: "several ARNs spanning two scopes of the same type",
            instrumentation_type: BREAKPOINT,
            arns: vec![Some(1), Some(5), Some(2)],
            deleted: vec![1, 5, 2],
        },
        Row {
            case: "a present ARN beside one that was never created",
            instrumentation_type: PROBE,
            arns: vec![Some(3), None],
            deleted: vec![3],
        },
        Row {
            case: "the same ARN listed twice",
            instrumentation_type: PROBE,
            arns: vec![Some(4), Some(4)],
            deleted: vec![4],
        },
        Row {
            case: "an ARN requested under the other instrumentation type",
            instrumentation_type: BREAKPOINT,
            arns: vec![Some(7)],
            deleted: vec![],
        },
        Row {
            case: "that same ARN under its own type",
            instrumentation_type: PROBE,
            arns: vec![Some(7)],
            deleted: vec![7],
        },
        Row {
            case: "an already deleted ARN beside the last live one",
            instrumentation_type: BREAKPOINT,
            arns: vec![Some(0), Some(6)],
            deleted: vec![6],
        },
    ];

    let mut gone: BTreeSet<usize> = BTreeSet::new();

    for row in &rows {
        let case = row.case;
        let listed: Vec<String> = row
            .arns
            .iter()
            .map(|entry| match entry {
                Some(index) => stored[*index].arn.clone(),
                None => missing.clone(),
            })
            .collect();
        let expected_deleted: Vec<String> = row
            .deleted
            .iter()
            .map(|index| stored[*index].arn.clone())
            .collect();
        // Every listed ARN the row does not expect deleted is an item that must fail, in the
        // order it was listed — including a repeat of an ARN the same request just deleted.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let expected_errors: Vec<String> = listed
            .iter()
            .filter(|arn| !expected_deleted.contains(arn) || !seen.insert((*arn).clone()))
            .cloned()
            .collect();

        let (status, answer) =
            batch_delete(&router, arn_list_target(row.instrumentation_type, &listed)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the batch delete, got {status} with {answer}"
        );
        assert_deleted_count(case, &answer, expected_deleted.len());
        assert_eq!(
            arn_deletions(case, &answer),
            expected_deleted,
            "case {case:?}: expected the resolved ARNs reported as deleted in request order, got {answer}"
        );

        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors
                .iter()
                .map(|error| error["ResourceArn"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            expected_errors,
            "case {case:?}: expected only the unresolved ARNs to fail, got {answer}"
        );
        for error in &errors {
            assert_eq!(
                error["Code"].as_str(),
                Some(RESOURCE_NOT_FOUND),
                "case {case:?}: expected the {RESOURCE_NOT_FOUND} code for an ARN that names nothing, got {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, CONFIGURATION_GONE,
                "case {case:?}: expected the per-item sentence {CONFIGURATION_GONE:?}, got {error}"
            );
            assert!(
                !message.contains("arn:"),
                "case {case:?}: expected the per-item message to name no ARN, got {error}"
            );
        }

        gone.extend(row.deleted.iter().copied());
        for (index, record) in stored.iter().enumerate() {
            assert_readable(&router, case, record, !gone.contains(&index)).await;
        }
    }

    assert_eq!(
        gone.len(),
        stored.len(),
        "expected the table to delete every stored configuration, got {gone:?}"
    );
}

/// An ARN-list selection resolves each listed ARN by the identity its RESOURCE field spells,
/// whatever PARTITION and REGION that ARN carries: this process holds one account's and one
/// Region's records, so those two fields render the writer's credential scope rather than any
/// part of a configuration's identity, and the modelled `InstrumentationConfigurationArn`
/// pattern (`^arn:[^:]+:application-signals:[^:]+:…`) leaves both unconstrained.
///
/// One request mixes the spellings: every configuration a listed ARN names goes and is
/// reported by the ARN that named it, while the items whose ARN names no stored configuration
/// of that type fail per item however their scope is written — a SECOND spelling of one the
/// same request has already deleted (the identity, not the spelling, is what makes it a
/// repeat), an ARN of another account, and an ARN naming another signal type.
#[tokio::test]
async fn an_arn_list_selection_resolves_each_arn_whatever_scope_it_carries() {
    let router = router();
    let stored = store_all(&router).await;
    let spellings = spellings();
    let elsewhere = spellings
        .last()
        .copied()
        .expect("the spelling table holds at least one row");

    // One row per instrumentation type, each listing several configurations spanning two
    // scopes. Indexes follow `definitions()`: 0..3 scope A (BREAKPOINT), 3..5 scope B (PROBE),
    // 5..7 scope C (BREAKPOINT), 7 scope D (PROBE) — so the two rows between them name every
    // stored configuration.
    let rows: [(&str, &str, &[usize]); 2] = [
        (
            "BREAKPOINT ARNs spanning two scopes",
            BREAKPOINT,
            &[0, 5, 1, 6, 2],
        ),
        ("PROBE ARNs spanning two scopes", PROBE, &[3, 7, 4]),
    ];

    let mut gone: BTreeSet<usize> = BTreeSet::new();
    for (case, instrumentation_type, indexes) in rows {
        // Each ARN of the list carries the next spelling in turn, so one request addresses its
        // configurations through several partitions and regions at once.
        let listed: Vec<String> = indexes
            .iter()
            .enumerate()
            .map(|(position, index)| {
                respelled(&stored[*index].arn, &spellings[position % spellings.len()])
            })
            .collect();

        // The ARNs that must fail their own item, in the order they are listed: the first
        // configuration again, under a spelling it was not listed with, and two ARNs differing
        // in a field that IS part of the identity — both also written with a foreign scope.
        let first = &stored[indexes[0]].arn;
        let repeated = respelled(first, &elsewhere);
        let other_account = respelled(&first.replace(ACCOUNT, "111111111111"), &elsewhere);
        let other_signal = respelled(&first.replace(SNAPSHOT, "OTHER_SIGNAL"), &elsewhere);
        assert_ne!(
            repeated, listed[0],
            "case {case:?}: expected the repeat to be spelled differently from the listing it repeats, got {repeated}"
        );

        let mut requested = listed.clone();
        requested.extend([
            repeated.clone(),
            other_account.clone(),
            other_signal.clone(),
        ]);

        let (status, answer) =
            batch_delete(&router, arn_list_target(instrumentation_type, &requested)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the batch delete, got {status} with {answer}"
        );
        assert_deleted_count(case, &answer, listed.len());
        assert_eq!(
            arn_deletions(case, &answer),
            listed,
            "case {case:?}: expected every spelling to delete the configuration it names, reported by the ARN that named it, got {answer}"
        );

        let expected_errors = vec![repeated, other_account, other_signal];
        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors
                .iter()
                .map(|error| error["ResourceArn"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            expected_errors,
            "case {case:?}: expected only the ARNs naming no stored configuration to fail, got {answer}"
        );
        for error in &errors {
            assert_eq!(
                error["Code"].as_str(),
                Some(RESOURCE_NOT_FOUND),
                "case {case:?}: expected the {RESOURCE_NOT_FOUND} code for an ARN that names nothing, got {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, CONFIGURATION_GONE,
                "case {case:?}: expected the per-item sentence {CONFIGURATION_GONE:?}, got {error}"
            );
            assert!(
                !message.contains("arn:"),
                "case {case:?}: expected the per-item message to name no ARN, got {error}"
            );
        }

        // The deletes landed on the configurations the spellings named, and on nothing else.
        gone.extend(indexes.iter().copied());
        for (index, record) in stored.iter().enumerate() {
            assert_readable(&router, case, record, !gone.contains(&index)).await;
        }
    }

    assert_eq!(
        gone.len(),
        stored.len(),
        "expected the table to name every stored configuration, got {gone:?}"
    );
}

/// A batch delete writes through to the store the other operations read: the identity it
/// freed takes a new configuration, which reads back as a record of its own with the tags of
/// the new create rather than the ones the deleted record carried.
#[tokio::test]
async fn an_identity_a_batch_delete_freed_takes_a_new_configuration() {
    let router = router();
    let stored = store_all(&router).await;
    let first = &stored[0];
    let case = "re-create after a scope delete";

    let (status, answer) = batch_delete(&router, SCOPE_A.deletion_target()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from the batch delete, got {status} with {answer}"
    );

    let (status, tags) = list_tags(&router, &first.arn).await;
    assert_eq!(
        status, 404,
        "case {case:?}: expected the deleted record's tags to be gone, got {status} with {tags}"
    );

    let mut body = first.definition.create_body();
    body["Tags"] = json!([{ "Key": "team", "Value": "checkout" }]);
    let (status, created) = create(&router, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the identity to be free for a new create, got {status} with {created}"
    );
    assert_eq!(
        created["ARN"].as_str(),
        Some(first.arn.as_str()),
        "case {case:?}: expected the same identity to mint the same ARN, got {created}"
    );

    let (status, fetched) = get(&router, first.get_body()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the new record to read back, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["Configuration"], created,
        "case {case:?}: expected the reader to answer the record the create wrote, got {fetched}"
    );

    let (status, tags) = list_tags(&router, &first.arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the new record's tags to be readable, got {status} with {tags}"
    );
    assert_eq!(
        tags["Tags"],
        json!([{ "Key": "team", "Value": "checkout" }]),
        "case {case:?}: expected the tags of the NEW create, got {tags}"
    );
}
