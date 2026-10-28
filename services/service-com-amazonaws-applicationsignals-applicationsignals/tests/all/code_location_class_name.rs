// SPDX-License-Identifier: CC0-1.0

//! The documented conditional requirement on `CodeLocation.ClassName`, over HTTP.
//!
//! The member is modelled OPTIONAL and its doc states the requirement the model cannot
//! express: "The class or type name that contains the method. This is required for Java and
//! optional for Python module-level functions"
//! (`smithy_query(query="doc",shape="CodeLocation",member="ClassName")`). So a code location
//! whose `Language` is the value that requires a class name and that names none describes no
//! instrumentation point, and every operation that accepts a code location refuses it with
//! the modelled `ValidationException` (HTTP 400) naming the member path and the requirement,
//! rather than hashing it and storing or resolving a record.
//!
//! The rule is asserted over a TABLE spanning what it generalises over: the whole modelled
//! `ProgrammingLanguage` catalogue, whether the location supplies a `ClassName`, which of
//! `CodeLocation`'s other optional members it supplies, and the published `length` bounds of
//! `ClassName` itself — and over EVERY operation that takes a code location, which is the
//! create (through `Location`) and the two reads and the delete (through
//! `LocationIdentifier`).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The `ProgrammingLanguage` that requires a class name, as `ClassName`'s own doc states it.
const JAVA: &str = "Java";

/// The published maximum of `ClassName` (`smithy_query(query="shape",shape="CodeLocation")`:
/// `ClassName` has `length 1..128`).
const CLASS_NAME_MAX: usize = 128;

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let payload = serde_json::to_vec(body).expect("serialize the request body");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `POST /create-instrumentation-configuration`.
async fn create(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "/create-instrumentation-configuration", body).await
}

/// `POST /list-instrumentation-configurations`.
async fn list(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "/list-instrumentation-configurations", body).await
}

/// The operations that take the instrumentation point as a `LocationIdentifier`, as
/// `(label, uri)`.
const IDENTIFIER_OPERATIONS: &[(&str, &str)] = &[
    ("get", "/get-instrumentation-configuration"),
    ("get status", "/get-instrumentation-configuration-status"),
    ("delete", "/delete-instrumentation-configuration"),
];

/// The scope every row is stored in. One row per router, so the scope never has to differ.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/namespace";

/// A `CaptureConfiguration` with only its `@required` members
/// (`smithy_query(query="shape",shape="CodeCaptureConfiguration,CaptureLimitsConfig")`).
fn minimal_capture() -> Value {
    json!({ "CodeCapture": { "CaptureLimits": {} } })
}

/// The refusal this service words for a code location under `member_path` that names no class
/// where its language requires one: the member path and the requirement, and nothing of the
/// value that was rejected.
fn class_name_refusal(member_path: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member_path}.codeLocation.className' failed \
         to satisfy constraint: Member must not be null for a {JAVA} code location"
    )
}

/// One row of the location table: the `CodeLocation` members a request carries and whether
/// the service may serve a request carrying them.
struct Case {
    label: &'static str,
    /// The `CodeLocation` body, `Language` included.
    code_location: Value,
    /// Whether the location names a class where its language requires one.
    served: bool,
}

/// The locations the rule is asserted over.
///
/// The rows differ along what the rule generalises over: the programming LANGUAGE (the whole
/// modelled catalogue — `smithy_query(query="shape",shape="ProgrammingLanguage")` ->
/// `["Java", "Python", "Javascript"]`, so `Javascript`, which neither the member doc nor the
/// report mentions, is covered too), whether the location supplies a `ClassName`, WHICH of
/// `CodeLocation`'s other optional members it supplies (none, some, all), and the published
/// `length 1..128` bounds of `ClassName` on both ends.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "Java, no ClassName",
            code_location: json!({
                "Language": JAVA,
                "FilePath": "src/main/java/com/example/CartService.java",
            }),
            served: false,
        },
        Case {
            label: "Java, no ClassName, every other optional member supplied",
            code_location: json!({
                "Language": JAVA,
                "CodeUnit": "com.example.checkout",
                "MethodName": "applyDiscount",
                "FilePath": "src/main/java/com/example/CartService.java",
                "LineNumber": 142,
            }),
            served: false,
        },
        Case {
            label: "Java, a ClassName",
            code_location: json!({
                "Language": JAVA,
                "ClassName": "com.example.CartService",
                "FilePath": "src/main/java/com/example/CartService.java",
                "LineNumber": 142,
            }),
            served: true,
        },
        Case {
            label: "Java, the shortest ClassName the model admits",
            code_location: json!({
                "Language": JAVA,
                "ClassName": "C",
                "FilePath": "src/main/java/com/example/CartService.java",
            }),
            served: true,
        },
        Case {
            label: "Java, the longest ClassName the model admits",
            code_location: json!({
                "Language": JAVA,
                "ClassName": "c".repeat(CLASS_NAME_MAX),
                "FilePath": "src/main/java/com/example/CartService.java",
            }),
            served: true,
        },
        Case {
            label: "Python, no ClassName: a module-level function",
            code_location: json!({
                "Language": "Python",
                "CodeUnit": "checkout_service",
                "MethodName": "apply_discount",
                "FilePath": "app/checkout/cart.py",
            }),
            served: true,
        },
        Case {
            label: "Python, a ClassName",
            code_location: json!({
                "Language": "Python",
                "ClassName": "Cart",
                "FilePath": "app/checkout/cart.py",
                "LineNumber": 1,
            }),
            served: true,
        },
        Case {
            label: "Javascript, no ClassName",
            code_location: json!({
                "Language": "Javascript",
                "FilePath": "web/src/cart.js",
            }),
            served: true,
        },
        Case {
            label: "Javascript, a ClassName",
            code_location: json!({
                "Language": "Javascript",
                "ClassName": "Cart",
                "MethodName": "total",
                "FilePath": "web/src/cart.js",
                "LineNumber": 7,
            }),
            served: true,
        },
    ]
}

/// The create body storing `code_location` under `instrumentation_type`.
fn create_body(code_location: &Value, instrumentation_type: &str) -> Value {
    json!({
        "InstrumentationType": instrumentation_type,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "Location": { "CodeLocation": code_location },
        "CaptureConfiguration": minimal_capture(),
    })
}

/// The body of an operation that identifies the point by `identifier`.
fn identifier_body(identifier: Value, instrumentation_type: &str) -> Value {
    json!({
        "InstrumentationType": instrumentation_type,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
        "SignalType": SNAPSHOT,
        "LocationIdentifier": identifier,
    })
}

/// The error code a refusal reports, with any namespace prefix the envelope carries dropped.
fn error_code(answer: &Value) -> Option<String> {
    answer["__type"]
        .as_str()
        .map(|kind| kind.rsplit('#').next().unwrap_or(kind).to_string())
}

/// Every operation that accepts a code location reads the same rule off it: a location of the
/// language that requires a class name and naming none is refused with HTTP 400
/// `ValidationException`, and every other location in the catalogue is served.
#[tokio::test]
async fn every_operation_accepting_a_code_location_applies_the_class_name_rule() {
    for case in cases() {
        let label = case.label;
        let router = router();

        let (status, answer) = create(&router, &create_body(&case.code_location, BREAKPOINT)).await;
        if case.served {
            assert_eq!(
                status, 200,
                "case {label:?}: expected the create to answer 200, got {status} with {answer}"
            );
            assert_eq!(
                answer["Location"]["CodeLocation"], case.code_location,
                "case {label:?}: expected the stored location to be the one created, got {:?}",
                answer["Location"]["CodeLocation"]
            );
        } else {
            assert_eq!(
                status, 400,
                "case {label:?}: expected the create to answer 400, got {status} with {answer}"
            );
            assert_eq!(
                error_code(&answer),
                Some("ValidationException".to_string()),
                "case {label:?}: expected a ValidationException, got {answer}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(class_name_refusal("location").as_str()),
                "case {label:?}: expected the refusal to name the member and the requirement, \
                 got {answer}"
            );
        }

        // The reads and the delete take the SAME location as a `LocationIdentifier`, so they
        // read the same rule — and a served location is reachable by the full location and by
        // the hash the create answered alike.
        let hash = answer["LocationHash"].as_str().unwrap_or_default().to_string();
        for (operation, uri) in IDENTIFIER_OPERATIONS {
            let identifiers: Vec<(&str, Value)> = if case.served && *operation != "delete" {
                vec![
                    ("by the code location", json!({ "CodeLocation": case.code_location })),
                    ("by the location hash", json!({ "LocationHash": hash })),
                ]
            } else {
                // The delete removes the record, so it is driven once — by the location
                // itself, which is the form carrying the rule under test.
                vec![(
                    "by the code location",
                    json!({ "CodeLocation": case.code_location }),
                )]
            };
            for (form, identifier) in identifiers {
                let (status, answer) =
                    call(&router, uri, &identifier_body(identifier, BREAKPOINT)).await;
                if case.served {
                    assert_eq!(
                        status, 200,
                        "case {label:?}: expected the {operation} {form} to answer 200, got \
                         {status} with {answer}"
                    );
                } else {
                    assert_eq!(
                        status, 400,
                        "case {label:?}: expected the {operation} {form} to answer 400, got \
                         {status} with {answer}"
                    );
                    assert_eq!(
                        error_code(&answer),
                        Some("ValidationException".to_string()),
                        "case {label:?}: expected the {operation} to answer a \
                         ValidationException, got {answer}"
                    );
                    assert_eq!(
                        answer["message"].as_str(),
                        Some(class_name_refusal("locationIdentifier").as_str()),
                        "case {label:?}: expected the {operation} refusal to name the member \
                         the request carried, got {answer}"
                    );
                }
            }
        }
    }
}

/// A refused create stores NOTHING: the scope it named still reports no configurations, and a
/// location that satisfies the rule is created at the same identity afterwards — so the
/// refusal left neither a record nor an occupied location behind.
#[tokio::test]
async fn a_refused_location_is_not_persisted() {
    let router = router();
    let without_class = json!({
        "Language": JAVA,
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 142,
    });
    let (status, refusal) = create(&router, &create_body(&without_class, PROBE)).await;
    assert_eq!(
        status, 400,
        "expected the create of a Java location with no ClassName to answer 400, got {status} \
         with {refusal}"
    );

    let scope = json!({
        "InstrumentationType": PROBE,
        "Service": SERVICE,
        "Environment": ENVIRONMENT,
    });
    let (status, listed) = list(&router, &scope).await;
    assert_eq!(
        status, 200,
        "expected the list to answer 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["LatestConfigurations"].as_array().map(Vec::len),
        Some(0),
        "expected the refused create to have stored nothing, got {listed}"
    );

    // The same point, now naming its class: created and read back.
    let mut with_class = without_class.clone();
    with_class["ClassName"] = json!("com.example.CartService");
    let (status, created) = create(&router, &create_body(&with_class, PROBE)).await;
    assert_eq!(
        status, 200,
        "expected the create naming a ClassName to answer 200, got {status} with {created}"
    );
    let hash = created["LocationHash"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        hash.len(),
        16,
        "expected the create to answer a 16-character LocationHash, got {created}"
    );

    let (status, fetched) = call(
        &router,
        "/get-instrumentation-configuration",
        &identifier_body(json!({ "LocationHash": hash }), PROBE),
    )
    .await;
    assert_eq!(
        status, 200,
        "expected the get to report the stored configuration, got {status} with {fetched}"
    );
    assert_eq!(
        fetched["Configuration"]["Location"]["CodeLocation"], with_class,
        "expected the get to report the location that was created, got {:?}",
        fetched["Configuration"]["Location"]["CodeLocation"]
    );
}

/// The rule keys on the LANGUAGE the location names, and on nothing else about the request:
/// the same refusal, with the same wording, whatever instrumentation type the create names
/// and whatever optional members it carries beside the location.
#[tokio::test]
async fn the_refusal_does_not_depend_on_the_rest_of_the_request() {
    let bare = json!({
        "Language": JAVA,
        "FilePath": "src/main/java/com/example/CartService.java",
    });
    let rows: Vec<(&str, Value)> = vec![
        ("BREAKPOINT, nothing optional", create_body(&bare, BREAKPOINT)),
        ("PROBE, nothing optional", create_body(&bare, PROBE)),
        (
            "BREAKPOINT, a description, filters and tags",
            {
                let mut body = create_body(&bare, BREAKPOINT);
                body["Description"] = json!("discount path snapshot");
                body["AttributeFilters"] = json!([{ "host.name": "ip-10-0-0-1" }]);
                body["Tags"] = json!([{ "Key": "team", "Value": "payments" }]);
                body
            },
        ),
        (
            "PROBE, another service and environment, the shortest the model admits",
            {
                let mut body = create_body(&bare, PROBE);
                body["Service"] = json!("a");
                body["Environment"] = json!("b");
                body
            },
        ),
    ];

    for (label, body) in rows {
        let router = router();
        let (status, answer) = create(&router, &body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400, got {status} with {answer}"
        );
        assert_eq!(
            answer["message"].as_str(),
            Some(class_name_refusal("location").as_str()),
            "case {label:?}: expected one wording for one rule, got {answer}"
        );
    }
}

/// The handler's refusal names the member exactly as the boundary validator names it, so a
/// client reads one member path for one member: `ClassName` outside its modelled
/// `length 1..128` is refused by the framework under the same path this rule refuses an
/// absent `ClassName` under.
///
/// The bounds are the published ones, tested on each end and one past each end.
#[tokio::test]
async fn the_modelled_class_name_bounds_report_the_same_member_path() {
    let rows: Vec<(&str, String, Option<&str>)> = vec![
        ("one character under the minimum", String::new(), Some(
            "Member must have length greater than or equal to 1",
        )),
        ("the minimum", "C".to_string(), None),
        ("the maximum", "c".repeat(CLASS_NAME_MAX), None),
        (
            "one character over the maximum",
            "c".repeat(CLASS_NAME_MAX + 1),
            Some("Member must have length less than or equal to 128"),
        ),
    ];

    for (label, class_name, requirement) in rows {
        let router = router();
        let code_location = json!({
            "Language": JAVA,
            "ClassName": class_name,
            "FilePath": "src/main/java/com/example/CartService.java",
        });
        let (status, answer) = create(&router, &create_body(&code_location, BREAKPOINT)).await;
        match requirement {
            None => assert_eq!(
                status, 200,
                "case {label:?}: expected a ClassName on its published bound to be served, got \
                 {status} with {answer}"
            ),
            Some(requirement) => {
                assert_eq!(
                    status, 400,
                    "case {label:?}: expected 400, got {status} with {answer}"
                );
                assert_eq!(
                    answer["message"].as_str(),
                    Some(
                        format!(
                            "1 validation error detected: Value at \
                             'location.codeLocation.className' failed to satisfy constraint: \
                             {requirement}"
                        )
                        .as_str()
                    ),
                    "case {label:?}: expected the modelled bound to be reported under the same \
                     member path, got {answer}"
                );
            }
        }
    }
}
