// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the tagging operations over an INSTRUMENTATION CONFIGURATION, the
//! service's second taggable resource.
//!
//! The rule under test: a tagging operation addresses a resource by the ARN that resource is
//! stored under, whatever KIND of resource of this service the ARN names — so the tags a
//! writer persisted for a resource (here `CreateInstrumentationConfiguration`'s `Tags`, under
//! the `instrumentationConfig/...` ARN it mints and answers in `ARN`) are exactly what
//! `ListTagsForResource` reports for that ARN and what `UntagResource` detaches from it,
//! while an ARN naming no resource this service still holds — one never created, one deleted,
//! one whose lifetime the clock has ended, one of another account — is the modelled
//! `ResourceNotFoundException` (404).
//!
//! Every assertion is made over a SET of configurations and tag sets rather than on one
//! request. The SLO side of the same rule lives in `resource_tags.rs`, whose request helpers
//! this suite reuses so both speak to one harness.

use std::collections::BTreeMap;
use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{expected_map, list_tags_raw, reported_tags, tag, tag_list, untag};
use crate::slo_lifecycle::{arn_uri_segment, call, ACCOUNT, REGION};

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// The capitalised type name a failure about an instrumentation configuration reports in
/// `ResourceType` (the resource's TYPE name, not the `instrumentationConfig` segment its ARN
/// spells).
const CONFIGURATION_RESOURCE_TYPE: &str = "InstrumentationConfiguration";

/// The resource-type segment of a configuration's ARN, from the modelled
/// `InstrumentationConfigurationArn` doc:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
const CONFIGURATION_ARN_RESOURCE: &str = "instrumentationConfig";

/// A router with just this service mounted on a fresh engine, together with the engine the
/// handlers share, so a test can stage what the clock would have done to a configuration
/// whose documented lifetime floor (5 minutes) outlasts any test.
fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    (router, engine)
}

/// `POST /create-instrumentation-configuration`.
async fn create_configuration(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/create-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /get-instrumentation-configuration`.
async fn get_configuration(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/get-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// `POST /delete-instrumentation-configuration`.
async fn delete_configuration(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        "/delete-instrumentation-configuration",
        Some(body),
    )
    .await
}

/// One row of the configuration table: the identity of a configuration to create and the tags
/// the create carries.
#[derive(Clone, Debug)]
struct Case {
    label: &'static str,
    instrumentation_type: &'static str,
    service: &'static str,
    environment: &'static str,
    /// The `CodeLocation` body of the `Location` union.
    location: Value,
    /// The `Tags` the create carries, or `None` when it names the member not at all.
    tags: Option<Vec<(String, String)>>,
}

impl Case {
    /// The create request this row stores.
    fn create_body(&self) -> Value {
        let mut body = json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "Location": { "CodeLocation": self.location },
            // `CaptureConfiguration`'s required union member, with only its own required
            // member (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`).
            "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } },
        });
        if let Some(tags) = self.tags.as_ref() {
            body["Tags"] = tag_list(tags);
        }
        body
    }

    /// The tags this row's configuration must report: the ones the create named, and none at
    /// all when it named the member not at all or named it empty.
    fn expected(&self) -> BTreeMap<String, String> {
        expected_map(self.tags.as_deref().unwrap_or_default())
    }

    /// A `GetInstrumentationConfiguration` request for this row's configuration, by the
    /// `LocationHash` the create answered.
    fn get_body(&self, location_hash: &str) -> Value {
        json!({
            "InstrumentationType": self.instrumentation_type,
            "Service": self.service,
            "Environment": self.environment,
            "SignalType": SNAPSHOT,
            "LocationIdentifier": { "LocationHash": location_hash },
        })
    }
}

/// `n` distinct tags, to assert the rule over a tag set the caller sizes.
fn generated_tags(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// The configurations and tag sets the rule is asserted over.
///
/// The rows differ along what the rule generalises over: the instrumentation TYPE (which
/// decides whether the configuration carries an expiry at all), the SERVICE and ENVIRONMENT
/// (which are spelled into the ARN the tags are addressed by, including an environment
/// carrying the `:` and `/` the modelled `^[A-Za-z0-9:/+=,.@_-]+$` pattern admits and the ARN
/// doc calls out), the programming LANGUAGE (the whole modelled `ProgrammingLanguage`
/// catalogue — `smithy_query(query="shape",shape="ProgrammingLanguage")` -> `["Java",
/// "Python", "Javascript"]`), and the shape of the tag set: how many tags it holds (none, one,
/// several, and the modelled `TagList` `length 0..200` maximum), whether the create named
/// `Tags` at all, and the sizes the modelled key and value admit
/// (`smithy_query(query="shape",shape="Tag,TagKey,TagValue")`: `TagKey` has `length 1..128`,
/// `TagValue` `length 0..256`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "BREAKPOINT, Java, several tags",
            instrumentation_type: BREAKPOINT,
            service: "checkout",
            environment: "eks:cluster-prod/namespace",
            location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/CartService.java",
                "LineNumber": 142,
            }),
            tags: Some(vec![
                tag("team", "checkout"),
                tag("env", "prod"),
                tag("tier", "gold"),
            ]),
        },
        Case {
            label: "PROBE, Python, one tag",
            instrumentation_type: PROBE,
            service: "pricing",
            environment: "ec2:production",
            location: json!({
                "Language": "Python",
                "FilePath": "app/pricing/quote.py",
                "MethodName": "quote",
            }),
            tags: Some(vec![tag("team", "pricing")]),
        },
        Case {
            label: "BREAKPOINT, Javascript, the empty tag list the model admits",
            instrumentation_type: BREAKPOINT,
            service: "web",
            environment: "default",
            location: json!({
                "Language": "Javascript",
                "FilePath": "src/checkout/cart.js",
                "ClassName": "Cart",
                "MethodName": "total",
                "LineNumber": 7,
            }),
            tags: Some(Vec::new()),
        },
        Case {
            label: "PROBE, Java, no Tags member at all",
            instrumentation_type: PROBE,
            service: "fulfilment",
            environment: "lambda:ship-order/live",
            location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/Ship.java",
            }),
            tags: None,
        },
        Case {
            label: "PROBE, Python, the longest key and an empty value",
            instrumentation_type: PROBE,
            service: "search",
            environment: "ecs:cluster/search-svc",
            location: json!({
                "Language": "Python",
                "CodeUnit": "search.index",
                "FilePath": "search/index.py",
                "ClassName": "Index",
                "MethodName": "rebuild",
                "LineNumber": 3001,
            }),
            tags: Some(vec![
                // The longest key the modelled `TagKey` `length 1..128` admits, the shortest
                // it admits, and the empty value `TagValue` `length 0..256` admits.
                tag(&"k".repeat(128), "longest-key"),
                tag("x", ""),
            ]),
        },
        Case {
            label: "BREAKPOINT, Java, the 200 tags TagList admits",
            instrumentation_type: BREAKPOINT,
            service: "inventory",
            environment: "eks:cluster-dev/inventory",
            location: json!({
                "Language": "Java",
                "FilePath": "src/main/java/com/example/Stock.java",
                "LineNumber": 1,
            }),
            tags: Some(generated_tags(200)),
        },
    ]
}

/// Create this row's configuration and hand back the `ARN` and `LocationHash` the service
/// answered — the stored record's own, never a value the test derived.
async fn created(router: &MultiServiceRouter, case: &Case) -> (String, String) {
    let (status, created) = create_configuration(router, case.create_body()).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from the create, got {status} with {created}",
        case.label
    );
    let arn = created["ARN"].as_str().unwrap_or_default().to_string();
    let hash = created["LocationHash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty() && !hash.is_empty(),
        "case {:?}: expected the create to answer an ARN and a LocationHash, got {created}",
        case.label
    );
    (arn, hash)
}

/// The tags a create persisted are what the reader reports for the configuration's own ARN,
/// for every shape of configuration identity and tag set — and a configuration created with
/// no tags reports the empty set rather than a failure.
#[tokio::test]
async fn created_tags_are_reported_for_the_configurations_own_arn() {
    let (router, _engine) = router_with_engine();
    for case in cases() {
        let (arn, _hash) = created(&router, &case).await;

        // The ARN the tags are addressed by is the one the modelled
        // `InstrumentationConfigurationArn` documents, which is what makes it a resource ARN
        // a tagging operation can resolve.
        let prefix =
            format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:{CONFIGURATION_ARN_RESOURCE}/");
        assert!(
            arn.starts_with(&prefix),
            "case {:?}: expected the minted ARN to start with {prefix:?}, got {arn:?}",
            case.label
        );

        let (status, body) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 reading the tags of {arn}, got {status} with {body}",
            case.label
        );
        assert_eq!(
            reported_tags(&body),
            case.expected(),
            "case {:?}: expected the created tags {:?}, got {:?}",
            case.label,
            case.expected(),
            reported_tags(&body)
        );
    }
}

/// The read reports and never writes: repeating it answers the same tags, and the
/// configuration itself is still served afterwards.
#[tokio::test]
async fn reading_the_tags_leaves_the_configuration_and_its_tags_alone() {
    let (router, _engine) = router_with_engine();
    for case in cases() {
        let (arn, hash) = created(&router, &case).await;
        let escaped = arn_uri_segment(&arn);

        let (_status, first) = list_tags_raw(&router, &escaped).await;
        for attempt in 1..=3 {
            let (status, again) = list_tags_raw(&router, &escaped).await;
            assert_eq!(
                status, 200,
                "case {:?} attempt {attempt}: expected HTTP 200 from the repeated read, got {status} with {again}",
                case.label
            );
            assert_eq!(
                reported_tags(&again),
                reported_tags(&first),
                "case {:?} attempt {attempt}: expected the read to report {:?}, got {:?}",
                case.label,
                reported_tags(&first),
                reported_tags(&again)
            );
        }

        let (status, fetched) = get_configuration(&router, case.get_body(&hash)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the configuration to survive the tag reads, got {status} with {fetched}",
            case.label
        );
    }
}

/// `UntagResource` resolves a configuration's ARN exactly as the reader does: it detaches the
/// keys the request names from that configuration and leaves its other tags in place, which
/// the reader then reports.
#[tokio::test]
async fn untagging_a_configuration_detaches_exactly_the_named_keys() {
    let (router, _engine) = router_with_engine();
    for case in cases() {
        let (arn, _hash) = created(&router, &case).await;
        let escaped = arn_uri_segment(&arn);

        // The first key the configuration carries, plus one it never carried: removal is per
        // key, so the absent one must change nothing.
        let mut expected = case.expected();
        let removed: Vec<String> = expected
            .keys()
            .next()
            .cloned()
            .into_iter()
            .chain(["never-attached".to_string()])
            .collect();
        for key in &removed {
            expected.remove(key);
        }

        let (status, body) = untag(&router, &arn, &removed).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the untag of {arn}, got {status} with {body}",
            case.label
        );

        let (status, after) = list_tags_raw(&router, &escaped).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from the post-untag read, got {status} with {after}",
            case.label
        );
        assert_eq!(
            reported_tags(&after),
            expected,
            "case {:?}: expected the remaining tags {expected:?}, got {:?}",
            case.label,
            reported_tags(&after)
        );
    }
}

/// A tag set belongs to ONE configuration: two configurations in the same scope, differing
/// only in the location they instrument, carry independent tag sets.
#[tokio::test]
async fn each_configuration_carries_its_own_tag_set() {
    let (router, _engine) = router_with_engine();
    let mut first = cases()
        .into_iter()
        .next()
        .expect("the table holds at least one configuration");
    first.tags = Some(vec![tag("team", "checkout"), tag("env", "prod")]);
    let mut second = first.clone();
    second.label = "the same scope at another location";
    second.location = json!({
        "Language": "Java",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 900,
    });
    second.tags = Some(vec![tag("team", "search")]);

    let (first_arn, _hash) = created(&router, &first).await;
    let (second_arn, _hash) = created(&router, &second).await;
    assert_ne!(
        first_arn, second_arn,
        "expected two locations to mint two ARNs, both {first_arn}"
    );

    for (case, arn) in [(&first, &first_arn), (&second, &second_arn)] {
        let (status, body) = list_tags_raw(&router, &arn_uri_segment(arn)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 reading {arn}, got {status} with {body}",
            case.label
        );
        assert_eq!(
            reported_tags(&body),
            case.expected(),
            "case {:?}: expected only its own tags {:?}, got {:?}",
            case.label,
            case.expected(),
            reported_tags(&body)
        );
    }

    // Untagging one leaves the other's identically-keyed tag alone.
    let (status, body) = untag(&router, &first_arn, &["team".to_string()]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the untag, got {status} with {body}"
    );
    let (_status, other) = list_tags_raw(&router, &arn_uri_segment(&second_arn)).await;
    assert_eq!(
        reported_tags(&other),
        expected_map(&[tag("team", "search")]),
        "expected the other configuration's tags to be untouched, got {:?}",
        reported_tags(&other)
    );
}

/// An ARN that names no configuration this service holds is the modelled
/// `ResourceNotFoundException` (404), identifying the resource by the ARN the request named
/// and by the type the ARN addresses — whether the configuration was never created, was
/// deleted, or belongs to another account.
#[tokio::test]
async fn an_arn_naming_no_stored_configuration_is_not_found() {
    let (router, _engine) = router_with_engine();
    let case = Case {
        tags: Some(vec![tag("team", "checkout")]),
        ..cases()
            .into_iter()
            .next()
            .expect("the table holds at least one configuration")
    };
    let (arn, hash) = created(&router, &case).await;

    // A second configuration, created and then deleted: its ARN named a resource once.
    let mut deleted_case = case.clone();
    deleted_case.label = "a deleted configuration";
    deleted_case.location = json!({
        "Language": "Java",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 7,
    });
    let (deleted_arn, deleted_hash) = created(&router, &deleted_case).await;
    let (status, body) = delete_configuration(&router, deleted_case.get_body(&deleted_hash)).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the setup delete, got {status} with {body}"
    );

    let never_created = arn.replace(&hash, &"0".repeat(16));
    let other_account = arn.replace(ACCOUNT, "111111111111");
    let other_service = arn.replace("application-signals", "cloudwatch");
    let missing = [
        ("a location nothing was created at", &never_created, true),
        ("a deleted configuration", &deleted_arn, true),
        ("another account's configuration", &other_account, true),
        ("another service's ARN", &other_service, false),
    ];

    for (label, resource_arn, names_a_configuration) in missing {
        let (status, error) = list_tags_raw(&router, &arn_uri_segment(resource_arn)).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected HTTP 404 for {resource_arn}, got {status} with {error}"
        );
        assert_eq!(
            error["__type"].as_str(),
            Some("ResourceNotFoundException"),
            "case {label:?}: expected a ResourceNotFoundException, got {error}"
        );
        assert_eq!(
            error["ResourceId"].as_str(),
            Some(resource_arn.as_str()),
            "case {label:?}: expected the refusal to name the ARN the request carried, got {error}"
        );
        if names_a_configuration {
            assert_eq!(
                error["ResourceType"].as_str(),
                Some(CONFIGURATION_RESOURCE_TYPE),
                "case {label:?}: expected the configuration type name, got {error}"
            );
        }

        // The refused read changed nothing: the live configuration still reports its tags.
        let (status, body) = list_tags_raw(&router, &arn_uri_segment(&arn)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the live configuration to still answer 200, got {status} with {body}"
        );
        assert_eq!(
            reported_tags(&body),
            case.expected(),
            "case {label:?}: expected a refused read to change nothing, got {:?}",
            reported_tags(&body)
        );
    }
}

/// A configuration whose `ExpiresAt` the clock has reached is "no longer served to clients",
/// so neither is its tag set: the read reports it as not found and the stored tags are gone,
/// while a configuration that does not expire keeps reporting its tags.
///
/// The documented floor on a BREAKPOINT lifetime is 5 minutes, so the passage of time is
/// staged on the store the handlers share, through `put_instrumentation_configuration` — the
/// very method the create path calls.
#[tokio::test]
async fn a_lapsed_configurations_tags_are_no_longer_reported() {
    let (router, engine) = router_with_engine();
    let service = "checkout";
    let environment = "eks:cluster-prod/namespace";
    let location = json!({
        "Language": "Java",
        "FilePath": "src/main/java/com/example/CartService.java",
        "LineNumber": 142,
    });
    let expiring = Case {
        label: "a BREAKPOINT configuration, which expires",
        instrumentation_type: BREAKPOINT,
        service,
        environment,
        location: location.clone(),
        tags: Some(vec![tag("team", "checkout")]),
    };
    // Another location under the type that "persists until explicitly deleted", whose own ARN
    // the lapse must leave alone. Its location differs, since the documented ARN format
    // (`instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`) names
    // the location rather than the instrumentation type.
    let permanent = Case {
        label: "a PROBE configuration, which does not expire",
        instrumentation_type: PROBE,
        tags: Some(vec![tag("team", "pricing")]),
        location: json!({
            "Language": "Java",
            "FilePath": "src/main/java/com/example/CartService.java",
            "LineNumber": 900,
        }),
        ..expiring.clone()
    };

    let (expiring_arn, hash) = created(&router, &expiring).await;
    let (permanent_arn, _hash) = created(&router, &permanent).await;

    let (status, body) = list_tags_raw(&router, &arn_uri_segment(&expiring_arn)).await;
    assert_eq!(
        status, 200,
        "expected the tags to be reported before the configuration lapses, got {status} with {body}"
    );

    // Stage what the clock will do: the same record, with an expiry it has already passed.
    let scope = InstrumentationScope::new(BREAKPOINT, service, environment);
    let key = instrumentation_key(SNAPSHOT, &hash);
    let lapsed_at = DateTime::from_secs_f64(
        DateTime::from(std::time::SystemTime::now())
            .as_secs_f64()
            .trunc()
            - 1.0,
    );
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let mut store = state.lock();
        let mut record = store
            .instrumentation_configuration(&scope, &key)
            .expect("the create stored the configuration under its identity")
            .clone();
        record.expires_at = Some(lapsed_at);
        store.put_instrumentation_configuration(scope.clone(), record, lapsed_at);
    }

    let (status, error) = list_tags_raw(&router, &arn_uri_segment(&expiring_arn)).await;
    assert_eq!(
        status, 404,
        "case {:?}: expected HTTP 404 once the lifetime ran out, got {status} with {error}",
        expiring.label
    );
    assert_eq!(
        error["__type"].as_str(),
        Some("ResourceNotFoundException"),
        "case {:?}: expected a ResourceNotFoundException, got {error}",
        expiring.label
    );

    // The lapse was materialised rather than merely hidden: the record and its tags are gone.
    {
        let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
        let store = state.lock();
        assert!(
            store.instrumentation_configuration(&scope, &key).is_none(),
            "case {:?}: expected the lapsed configuration to be dropped from the store",
            expiring.label
        );
        assert!(
            !store.tags.contains_key(&expiring_arn),
            "case {:?}: expected the lapsed configuration's tags to be dropped, got {:?}",
            expiring.label,
            store.tags.get(&expiring_arn)
        );
    }

    let (status, body) = list_tags_raw(&router, &arn_uri_segment(&permanent_arn)).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected a configuration that does not expire to keep its tags, got {status} with {body}",
        permanent.label
    );
    assert_eq!(
        reported_tags(&body),
        permanent.expected(),
        "case {:?}: expected {:?}, got {:?}",
        permanent.label,
        permanent.expected(),
        reported_tags(&body)
    );
}
