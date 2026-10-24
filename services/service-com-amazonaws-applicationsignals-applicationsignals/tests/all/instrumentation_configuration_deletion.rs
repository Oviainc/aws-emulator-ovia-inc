// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of removing a dynamic instrumentation configuration over HTTP:
//! `DeleteInstrumentationConfiguration` and the `GetInstrumentationConfiguration` reader
//! that answers what is left.
//!
//! The rule under test: a configuration is addressed by its SERVICE, ENVIRONMENT,
//! INSTRUMENTATION TYPE, SIGNAL TYPE and LOCATION; a read answers the stored record for
//! either form of `LocationIdentifier`; and a delete removes exactly the configuration its
//! identity names and nothing else — after which that identity holds none, so a reader and a
//! second delete are both refused with the modelled `ResourceNotFoundException`, the
//! record's tags are gone with it, the scope's set reports the change, and the identity is
//! free for a new create.
//!
//! Every assertion is made over a TABLE of identities and configuration shapes rather than
//! on one request. `instrumentation_configurations` covers the create and the list; the
//! request helpers are the ones `slo_lifecycle` already drives this service's `restJson1`
//! surface with.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{arn_uri_segment, call, router, ACCOUNT, REGION};

/// The `ResourceType` every failure about an instrumentation configuration reports: the
/// resource's type NAME, capitalised
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")` makes both
/// `ResourceType` and `ResourceId` `@required`).
const RESOURCE_TYPE: &str = "InstrumentationConfiguration";

/// The only `DynamicInstrumentationDeletionStatus` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationDeletionStatus")` ->
/// `["DELETED"]`).
const DELETED: &str = "DELETED";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` ->
/// `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values the model declares
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

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

/// `POST /delete-instrumentation-configuration`.
async fn delete(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/delete-instrumentation-configuration",
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

/// `GET /tags?ResourceArn=<arn>` — the reader that reports what a create stored for a
/// resource's tags.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/tags?ResourceArn={}", arn_uri_segment(arn)),
        None,
    )
    .await
}

/// Now, as the epoch seconds a `Timestamp` member carries on `restJson1`.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One row of the configuration table: an identity, and the configuration stored under it.
struct Case {
    label: &'static str,
    instrumentation_type: &'static str,
    service: &'static str,
    environment: &'static str,
    /// The modelled `Location` union, whose only member is a `CodeLocation`.
    location: Value,
    description: Option<&'static str>,
    attribute_filters: Option<Value>,
    capture_configuration: Value,
    tags: Option<Value>,
}

impl Case {
    /// The `CreateInstrumentationConfiguration` request body for this row.
    fn create_body(&self) -> Value {
        let mut body = json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "Location": self.location,
            "CaptureConfiguration": self.capture_configuration,
        });
        if let Some(description) = self.description {
            body["Description"] = json!(description);
        }
        if let Some(filters) = self.attribute_filters.as_ref() {
            body["AttributeFilters"] = filters.clone();
        }
        if let Some(tags) = self.tags.as_ref() {
            body["Tags"] = tags.clone();
        }
        body
    }

    /// A `Get`/`Delete` request body for this row, addressing the configuration by the
    /// `LocationIdentifier` form `identifier` carries and by `instrumentation_type`.
    fn read_body(&self, instrumentation_type: &str, identifier: Value) -> Value {
        json!({
            "InstrumentationType": instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "LocationIdentifier": identifier,
        })
    }

    /// A `ListInstrumentationConfigurations` request body for this row's scope.
    fn list_body(&self) -> Value {
        json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
        })
    }

    /// The `LocationIdentifier` that names this row's location in full ("either full code
    /// location or a pre-computed hash").
    fn by_location(&self) -> Value {
        json!({ "CodeLocation": self.location["CodeLocation"].clone() })
    }

    /// The `LocationIdentifier` that names this row's location by the hash a create answered.
    fn by_hash(&self, location_hash: &str) -> Value {
        json!({ "LocationHash": location_hash })
    }

    /// The ARN this row's configuration carries, given the location hash the create computed:
    /// the format the modelled `InstrumentationConfigurationArn` documents,
    /// `…:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
    fn arn(&self, location_hash: &str) -> String {
        format!(
            "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/{}/{}/{SNAPSHOT}/{location_hash}",
            self.service, self.environment
        )
    }
}

/// A `CaptureConfiguration`: the modelled union's only member is `CodeCapture`, whose only
/// `@required` member is `CaptureLimits`
/// (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`).
fn capture(max_hits: i64) -> Value {
    json!({
        "CodeCapture": {
            "CaptureReturn": true,
            "CaptureLimits": { "MaxHits": max_hits }
        }
    })
}

/// A `Location` carrying a `CodeLocation` in `language`.
fn code_location(language: &str, members: Value) -> Value {
    let mut code = members;
    code["Language"] = json!(language);
    json!({ "CodeLocation": code })
}

/// The configurations the rule is asserted over.
///
/// The rows differ along what the rule generalises over: the instrumentation TYPE (hence the
/// scope a configuration lives in, and whether it expires at all), the programming LANGUAGE
/// (the whole modelled catalogue — `smithy_query(query="shape",shape="ProgrammingLanguage")`
/// -> `["Java", "Python", "Javascript"]`), WHICH of `CodeLocation`'s optional members the
/// location supplies and what they hold (including its published `length`/`range` bounds),
/// how many attribute filter groups target the instrumentation
/// (`DynamicInstrumentationAttributeFilters`, `length 1..10`), whether a description and tags
/// are attached, and the shape of the service and environment names
/// (`^[A-Za-z0-9:/+=,.@_-]+$`, `length 1..255`, so both may carry `:` and `/`).
///
/// Every row names a DIFFERENT identity, so one store holds them all at once — and the rows
/// deliberately overlap in parts of it: two share a service and environment, two share a
/// location, and one repeats another's location in a different environment.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "breakpoint, every code-location member, filters, description and tags",
            instrumentation_type: BREAKPOINT,
            service: "checkout",
            environment: "eks:cluster-prod/namespace",
            location: code_location(
                "Java",
                json!({
                    "CodeUnit": "checkout-service",
                    "ClassName": "com.example.checkout.CartController",
                    "MethodName": "applyDiscount",
                    "FilePath": "src/main/java/com/example/checkout/CartController.java",
                    "LineNumber": 142
                }),
            ),
            description: Some("discount path snapshot"),
            attribute_filters: Some(json!([
                { "host.name": "ip-10-0-0-1", "deployment.version": "3.1.4" },
                { "host.name": "ip-10-0-0-2" }
            ])),
            capture_configuration: capture(5),
            tags: Some(json!([
                { "Key": "team", "Value": "payments" },
                { "Key": "env", "Value": "prod" }
            ])),
        },
        Case {
            // Only the members the model marks `@required`: `Language` and `FilePath`.
            label: "probe, required code-location members only",
            instrumentation_type: PROBE,
            service: "inventory",
            environment: "ec2:production",
            location: code_location("Python", json!({ "FilePath": "app/inventory/stock.py" })),
            description: None,
            attribute_filters: None,
            capture_configuration: capture(1),
            tags: None,
        },
        Case {
            // The published bounds of the location's own members: `ClassName` at its `length`
            // minimum of 1, `MethodName` at its maximum of 80, `FilePath` at its maximum of
            // 1024, `LineNumber` at its `range` minimum of 1 — and the shortest service and
            // environment names the modelled `length 1..255` admits.
            label: "breakpoint, code-location members on their published bounds",
            instrumentation_type: BREAKPOINT,
            service: "a",
            environment: "e",
            location: code_location(
                "Javascript",
                json!({
                    "CodeUnit": "c",
                    "ClassName": "C",
                    "MethodName": "m".repeat(80),
                    "FilePath": format!("src/{}.js", "p".repeat(1017)),
                    "LineNumber": 1
                }),
            ),
            description: Some("x"),
            attribute_filters: Some(json!([{ "k": "v" }])),
            capture_configuration: capture(1000),
            tags: Some(json!([{ "Key": "only", "Value": "one" }])),
        },
        Case {
            // The SAME file as the first row, in the same service and environment, one line
            // further down and under the other instrumentation type: a different identity, so
            // a different configuration.
            label: "probe, same file as another configuration but another line",
            instrumentation_type: PROBE,
            service: "checkout",
            environment: "eks:cluster-prod/namespace",
            location: code_location(
                "Java",
                json!({
                    "CodeUnit": "checkout-service",
                    "ClassName": "com.example.checkout.CartController",
                    "MethodName": "applyDiscount",
                    "FilePath": "src/main/java/com/example/checkout/CartController.java",
                    "LineNumber": 143
                }),
            ),
            description: None,
            // The modelled maximum of `length 1..10` filter groups.
            attribute_filters: Some(json!([
                { "f0": "0" }, { "f1": "1" }, { "f2": "2" }, { "f3": "3" }, { "f4": "4" },
                { "f5": "5" }, { "f6": "6" }, { "f7": "7" }, { "f8": "8" }, { "f9": "9" }
            ])),
            capture_configuration: capture(1000),
            tags: None,
        },
        Case {
            // The first row's location again, in a DIFFERENT environment: the location hash
            // identifies the point only within the service, environment and signal type, so
            // this is another configuration carrying the SAME hash.
            label: "breakpoint, another environment of the same service and location",
            instrumentation_type: BREAKPOINT,
            service: "checkout",
            environment: "ec2:staging",
            location: code_location(
                "Java",
                json!({
                    "CodeUnit": "checkout-service",
                    "ClassName": "com.example.checkout.CartController",
                    "MethodName": "applyDiscount",
                    "FilePath": "src/main/java/com/example/checkout/CartController.java",
                    "LineNumber": 142
                }),
            ),
            description: None,
            attribute_filters: None,
            capture_configuration: capture(2),
            tags: Some(json!([{ "Key": "team", "Value": "payments" }])),
        },
    ]
}

/// The `LocationHash` a create answered, checked to be the 16 lower-case hexadecimal digits
/// the modelled `length 16..16` and the ARN pattern's `[0-9a-f]{16}` require.
fn location_hash(label: &str, created: &Value) -> String {
    let hash = created["LocationHash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        hash.len() == 16
            && hash
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
        "case {label:?}: expected 16 lower-case hexadecimal digits of LocationHash, got {hash:?}"
    );
    hash
}

/// Create every row and answer the location hash each create reported, in row order.
async fn create_all(router: &MultiServiceRouter, cases: &[Case]) -> Vec<String> {
    let mut hashes = Vec::new();
    for case in cases {
        let (status, created) = create(router, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup create to succeed, got {status} with {created}",
            case.label
        );
        hashes.push(location_hash(case.label, &created));
    }
    hashes
}

/// Assert that `answer` is the modelled `ResourceNotFoundException` for the configuration
/// `arn` names.
fn assert_not_found(case: &str, what: &str, status: u16, answer: &Value, arn: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected HTTP 404 from {what}, got {status} with {answer}"
    );
    assert_eq!(
        answer["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {case:?}: expected a ResourceNotFoundException from {what}, got {answer}"
    );
    assert_eq!(
        answer["ResourceType"].as_str(),
        Some(RESOURCE_TYPE),
        "case {case:?}: expected ResourceType {RESOURCE_TYPE:?} from {what}, got {answer}"
    );
    assert_eq!(
        answer["ResourceId"].as_str(),
        Some(arn),
        "case {case:?}: expected {what} to identify the configuration by its ARN {arn:?}, got {answer}"
    );
}

/// The ARNs a `ListInstrumentationConfigurations` page reports, in the order it reports them.
fn listed_arns(page: &Value) -> Vec<String> {
    page["LatestConfigurations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["ARN"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A delete removes exactly the configuration its identity names: it answers `DELETED`, the
/// record and its tags are gone (a read, a second delete and a tag read are all 404), every
/// other stored configuration is untouched, and the identity is free for a new create.
///
/// The read before each delete is the round trip: whichever form of `LocationIdentifier`
/// names the record, the reader answers the record the create wrote.
#[tokio::test]
async fn a_delete_removes_only_the_configuration_it_names() {
    let router = router();
    let cases = cases();
    let hashes = create_all(&router, &cases).await;

    for (deleted, case) in cases.iter().enumerate() {
        let hash = &hashes[deleted];
        let arn = case.arn(hash);

        // Both `LocationIdentifier` forms address one record.
        for (form, identifier) in [
            ("the full code location", case.by_location()),
            ("a pre-computed hash", case.by_hash(hash)),
        ] {
            let (status, read) = get(
                &router,
                case.read_body(case.instrumentation_type, identifier),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?} by {form}: expected HTTP 200 from the read, got {status} with {read}",
                case.label
            );
            assert_eq!(
                read["Configuration"]["ARN"].as_str(),
                Some(arn.as_str()),
                "case {:?} by {form}: expected the record stored at {arn:?}, got {:?}",
                case.label,
                read["Configuration"]
            );
        }

        // The deletes alternate between the two identifier forms, so each addresses the
        // record the other one read.
        let identifier = if deleted % 2 == 0 {
            case.by_hash(hash)
        } else {
            case.by_location()
        };
        let (status, answer) = delete(
            &router,
            case.read_body(case.instrumentation_type, identifier.clone()),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the delete, got {status} with {answer}",
            case.label
        );
        assert_eq!(
            answer["DeletionStatus"].as_str(),
            Some(DELETED),
            "case {:?}: expected DeletionStatus {DELETED:?}, got {answer}",
            case.label
        );

        // The record is gone, so the reader and a second delete both refuse — the operation
        // is not `@idempotent` and models `ResourceNotFoundException`.
        let (status, error) = get(
            &router,
            case.read_body(case.instrumentation_type, identifier.clone()),
        )
        .await;
        assert_not_found(case.label, "a read after the delete", status, &error, &arn);
        let (status, error) = delete(
            &router,
            case.read_body(case.instrumentation_type, identifier.clone()),
        )
        .await;
        assert_not_found(case.label, "a second delete", status, &error, &arn);

        // The tag set went with the record: nothing is addressed by that ARN any more.
        let (status, error) = list_tags(&router, &arn).await;
        assert_not_found(
            case.label,
            "a tag read after the delete",
            status,
            &error,
            &arn,
        );

        // Every configuration this delete did not name is still stored, unchanged.
        for (survivor, other) in cases
            .iter()
            .enumerate()
            .filter(|(index, _)| *index > deleted)
        {
            let (status, read) = get(
                &router,
                other.read_body(other.instrumentation_type, other.by_hash(&hashes[survivor])),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected {:?} to survive the delete, got {status} with {read}",
                case.label, other.label
            );
            assert_eq!(
                read["Configuration"]["ARN"].as_str(),
                Some(other.arn(&hashes[survivor]).as_str()),
                "case {:?}: expected {:?} to be unchanged by the delete, got {:?}",
                case.label,
                other.label,
                read["Configuration"]
            );
        }

        // The identity holds nothing now, so a create under it is accepted again rather than
        // conflicting — and the configuration it mints is removable in turn.
        let (status, recreated) = create(&router, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the identity to be free after the delete, got {status} with {recreated}",
            case.label
        );
        let (status, answer) = delete(
            &router,
            case.read_body(case.instrumentation_type, case.by_hash(hash)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the re-created configuration to delete too, got {status} with {answer}",
            case.label
        );
    }
}

/// `InstrumentationType` is "required to determine which backing store to query", so a
/// delete naming the other type names no configuration: it is refused with 404 and removes
/// nothing.
#[tokio::test]
async fn a_delete_naming_the_other_instrumentation_type_removes_nothing() {
    for case in cases() {
        let router = router();
        let (status, created) = create(&router, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup create to succeed, got {status} with {created}",
            case.label
        );
        let hash = location_hash(case.label, &created);
        let other_type = if case.instrumentation_type == BREAKPOINT {
            PROBE
        } else {
            BREAKPOINT
        };

        let (status, error) = delete(&router, case.read_body(other_type, case.by_hash(&hash))).await;
        assert_eq!(
            status, 404,
            "case {:?}: expected HTTP 404 deleting as {other_type}, got {status} with {error}",
            case.label
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {:?}: expected a ResourceNotFoundException, got {error}",
            case.label
        );

        // The configuration the request did not name is still there, and still deletable.
        let (status, read) = get(
            &router,
            case.read_body(case.instrumentation_type, case.by_hash(&hash)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the configuration to survive a delete of the other type, got {status} with {read}",
            case.label
        );
        assert_eq!(
            read["Configuration"], created,
            "case {:?}: expected the record to be unchanged, got {:?}",
            case.label, read["Configuration"]
        );

        let (status, answer) = delete(
            &router,
            case.read_body(case.instrumentation_type, case.by_hash(&hash)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the delete of the stored type to succeed, got {status} with {answer}",
            case.label
        );
    }
}

/// An identity that holds no configuration is refused with the modelled
/// `ResourceNotFoundException` and removes nothing — for identities that differ from a stored
/// one in exactly ONE component, and for both forms of `LocationIdentifier`.
#[tokio::test]
async fn a_delete_of_an_unknown_identity_is_not_found() {
    let router = router();
    let cases = cases();
    let stored = &cases[0];
    let (status, created) = create(&router, stored.create_body()).await;
    assert_eq!(
        status, 200,
        "expected the setup create to succeed, got {status} with {created}"
    );
    let hash = location_hash(stored.label, &created);
    let stored_location = stored.location["CodeLocation"].clone();

    /// One identity component swapped out of the stored identity.
    fn with(body: Value, member: &str, value: Value) -> Value {
        let mut body = body;
        body[member] = value;
        body
    }
    let base = stored.read_body(stored.instrumentation_type, stored.by_hash(&hash));

    let rows = [
        (
            "another service",
            with(base.clone(), "Service", json!("another-service")),
        ),
        (
            "another environment",
            with(base.clone(), "Environment", json!("ec2:another")),
        ),
        (
            "a hash no configuration carries",
            with(
                base.clone(),
                "LocationIdentifier",
                stored.by_hash("0123456789abcdef"),
            ),
        ),
        (
            "a code location one line further down",
            with(
                base.clone(),
                "LocationIdentifier",
                json!({ "CodeLocation": with(stored_location.clone(), "LineNumber", json!(143)) }),
            ),
        ),
        (
            "a code location whose method name differs",
            with(
                base.clone(),
                "LocationIdentifier",
                json!({
                    "CodeLocation": with(
                        stored_location.clone(),
                        "MethodName",
                        json!("applyDiscounts"),
                    )
                }),
            ),
        ),
        (
            "a code location missing one optional member",
            with(base.clone(), "LocationIdentifier", {
                let mut location = stored_location.clone();
                location
                    .as_object_mut()
                    .expect("the stored location is an object")
                    .remove("CodeUnit");
                json!({ "CodeLocation": location })
            }),
        ),
    ];

    for (label, body) in rows {
        let (status, error) = delete(&router, body).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for an identity holding no configuration, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
    }

    // None of those refusals touched the stored configuration.
    let (status, read) = get(&router, base).await;
    assert_eq!(
        status, 200,
        "expected the stored configuration to survive every refused delete, got {status} with {read}"
    );
    assert_eq!(
        read["Configuration"], created,
        "expected the stored record to be unchanged, got {:?}",
        read["Configuration"]
    );
}

/// A delete is a CHANGE to its scope's set, so the list that reports the scope stops
/// reporting the deleted configuration and tells a caller that synced before the delete that
/// something changed — while a caller that synced after it is told nothing did.
#[tokio::test]
async fn a_delete_is_reported_by_the_list_of_its_scope() {
    let router = router();
    let cases = cases();
    // The two rows that share a service and an environment but differ in instrumentation
    // type live in DIFFERENT scopes, so each list reports one of them.
    let kept = &cases[0];
    let other_scope = &cases[3];
    for case in [kept, other_scope] {
        let (status, created) = create(&router, case.create_body()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup create to succeed, got {status} with {created}",
            case.label
        );
    }

    let (status, page) = list(&router, kept.list_body()).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the list, got {status} with {page}"
    );
    assert_eq!(
        listed_arns(&page).len(),
        1,
        "expected the scope to report its one configuration, got {page}"
    );

    let (status, page) = list(&router, other_scope.list_body()).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the other scope's list, got {status} with {page}"
    );
    assert_eq!(
        listed_arns(&page).len(),
        1,
        "expected the other scope to report its one configuration, got {page}"
    );

    let before_delete = now_epoch_seconds();
    let (status, answer) = delete(
        &router,
        other_scope.read_body(other_scope.instrumentation_type, other_scope.by_location()),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the delete to succeed, got {status} with {answer}"
    );

    // The deleted configuration's scope no longer reports it, and the change is news to a
    // caller that synced before the delete.
    let (status, page) = list(&router, other_scope.list_body()).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the list after the delete, got {status} with {page}"
    );
    assert_eq!(
        listed_arns(&page),
        Vec::<String>::new(),
        "expected the deleted configuration to be gone from its scope, got {page}"
    );

    let mut incremental = other_scope.list_body();
    incremental["SyncedAt"] = json!(before_delete);
    let (status, page) = list(&router, incremental).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the incremental list, got {status} with {page}"
    );
    assert_eq!(
        page["Changed"].as_bool(),
        Some(true),
        "expected a sync from before the delete to be told the set changed, got {page}"
    );

    let mut after = other_scope.list_body();
    after["SyncedAt"] = json!(now_epoch_seconds());
    let (status, page) = list(&router, after).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the later incremental list, got {status} with {page}"
    );
    assert_eq!(
        page["Changed"].as_bool(),
        Some(false),
        "expected a sync from after the delete to be told nothing changed, got {page}"
    );

    // The other scope's configuration is untouched: a delete removes one record, not a
    // service's instrumentation.
    let (status, page) = list(&router, kept.list_body()).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the kept scope's list, got {status} with {page}"
    );
    assert_eq!(
        listed_arns(&page).len(),
        1,
        "expected the other scope to keep its configuration, got {page}"
    );
}
