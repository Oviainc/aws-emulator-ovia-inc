// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal, asserted over the set of inputs each rule holds for
//! rather than on one request.
//!
//! Three rules:
//!
//! 1. A refusal that an instrumentation configuration cannot be resolved carries `Message`
//!    alone — fixed prose naming no ARN, service, environment, signal type or location hash,
//!    because the request already carries the identity it asked about. WHICH sentence it is
//!    follows the path and what the store remembers of the location: a READ reports a
//!    configuration that was removed from the location (by a delete or by the lapse of its
//!    `ExpiresAt`) as deleted and a location nothing was stored at as holding none, while a
//!    DELETE reports both as holding none — it states what this call could not do, not what
//!    became of an earlier configuration.
//! 2. A second create at one identity conflicts on the four identity DIMENSIONS the
//!    uniqueness rule is on (service, environment, signal type and location) and names no
//!    value of any of them.
//! 3. `UpdateServiceLevelObjective` resolves an SLO through the work item that holds it, so
//!    its refusal of an `Id` that names no stored SLO is worded by that layer: the account,
//!    the SLO NAME taken from the request (whichever id form it used), the item's own
//!    12-digit discriminator and the namespace the item lives in. The resource-layer
//!    wording the other SLO reads use is asserted in `error_identity`.
//!
//! The request helpers mirror the ones the sibling instrumentation and SLO suites drive this
//! service's `restJson1` surface with.

use std::sync::Arc;

use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use service_com_amazonaws_applicationsignals_applicationsignals::state::{
    instrumentation_key, ComAmazonawsApplicationsignalsApplicationsignalsState,
    InstrumentationScope,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::call as post;
use crate::instrumentation_configuration_tags::router_with_engine;
use crate::slo_lifecycle::{
    arn_path_segment, call as slo_call, create as slo_create, name_path_segment, sli_config,
    router as slo_router, ACCOUNT,
};

/// The region the sibling suites' request helper signs for, which is the one a minted ARN
/// would carry — so a message that names no ARN never names it either.
const REGION: &str = "eu-west-2";

/// The only `DynamicInstrumentationSignalType` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationSignalType")` -> `["SNAPSHOT"]`).
const SNAPSHOT: &str = "SNAPSHOT";

/// The two `InstrumentationType` values the model declares
/// (`smithy_query(query="shape",shape="InstrumentationType")` -> `["BREAKPOINT", "PROBE"]`).
const BREAKPOINT: &str = "BREAKPOINT";
const PROBE: &str = "PROBE";

/// A `LocationHash` no create ever minted: 16 lowercase hex characters, as the member's
/// `length 16..16` and the `InstrumentationConfigurationArn` pattern's `[0-9a-f]{16}` suffix
/// require.
const UNMINTED_HASH: &str = "0f1e2d3c4b5a6978";

/// The sentence a read of a location a configuration was REMOVED from is refused with.
pub(crate) const CONFIGURATION_DELETED: &str =
    "The requested instrumentation configuration has been deleted.";

/// The sentence a refusal about a location holding no configuration, and one a delete it
/// cannot resolve is refused with, states.
pub(crate) const CONFIGURATION_ABSENT: &str =
    "The requested instrumentation configuration does not exist.";

/// The sentence a second create at one identity is refused with: the four identity
/// dimensions, and no value of any of them.
pub(crate) const CONFIGURATION_CONFLICT: &str =
    "An instrumentation configuration already exists for this service, environment, \
     signalType and location.";

/// The fixed tail of the work-item refusal `UpdateServiceLevelObjective` answers, including
/// the namespace an SLO work item lives in and no trailing period.
const WORK_ITEM_SUFFIX: &str = " and namespace: slo does not exist";

/// How many digits the work-item discriminator carries.
const WORK_ITEM_DISCRIMINATOR_DIGITS: usize = 12;

/// One row of the configuration table: the create that stores it, and which form of
/// `LocationIdentifier` the later requests address it by.
struct Case {
    label: &'static str,
    request: Value,
    /// Whether the requests name the location by its pre-computed hash rather than by the
    /// full code location. Both forms resolve one record, so both must be refused alike.
    by_hash: bool,
}

/// The identities rule 1 and rule 2 are asserted over.
///
/// The rows differ along what the rules generalise over: both `InstrumentationType` values,
/// the whole modelled `ProgrammingLanguage` catalogue
/// (`smithy_query(query="shape",shape="ProgrammingLanguage")` -> `["Java", "Python",
/// "Javascript"]`), which of `CodeLocation`'s optional members the location carries (the
/// fullest and the smallest the model admits), the spelling of the service and environment
/// names (including the `:` and `/` the `@pattern` admits), and which form of
/// `LocationIdentifier` the request uses.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "BREAKPOINT, Java, every optional location member, by hash",
            request: json!({
                "Service": "checkout",
                "Environment": "eks:cluster-prod/namespace",
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                "Description": "capture the cart total",
                "Location": { "CodeLocation": {
                    "Language": "Java",
                    "CodeUnit": "checkout-service.jar",
                    "ClassName": "com.example.CartService",
                    "MethodName": "total",
                    "FilePath": "src/main/java/com/example/CartService.java",
                    "LineNumber": 142
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": { "MaxHits": 3 } } }
            }),
            by_hash: true,
        },
        Case {
            label: "PROBE, Python, smallest location, by location",
            request: json!({
                "Service": "orders",
                "Environment": "prod",
                "SignalType": SNAPSHOT,
                "InstrumentationType": PROBE,
                "Location": { "CodeLocation": {
                    "Language": "Python",
                    "FilePath": "app/orders.py"
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": {} } }
            }),
            by_hash: false,
        },
        Case {
            label: "BREAKPOINT, Javascript, dotted names, by location",
            request: json!({
                "Service": "cart.web.frontend",
                "Environment": "ec2:staging-1",
                "SignalType": SNAPSHOT,
                "InstrumentationType": BREAKPOINT,
                "Location": { "CodeLocation": {
                    "Language": "Javascript",
                    "MethodName": "render",
                    "FilePath": "src/cart.js",
                    "LineNumber": 7
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": { "MaxHits": 1 } } }
            }),
            by_hash: false,
        },
        Case {
            label: "PROBE, Java, long file path, by hash",
            request: json!({
                "Service": "shipping",
                "Environment": "lambda:prod",
                "SignalType": SNAPSHOT,
                "InstrumentationType": PROBE,
                "Location": { "CodeLocation": {
                    "Language": "Java",
                    "ClassName": "com.example.shipping.label.LabelPrinter",
                    "FilePath": "src/main/java/com/example/shipping/label/LabelPrinter.java"
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": { "MaxHits": 9 } } }
            }),
            by_hash: true,
        },
    ]
}

impl Case {
    /// The `LocationIdentifier` a request addresses this row's location by, given the hash
    /// the create reported for it.
    fn identifier(&self, hash: &str) -> Value {
        if self.by_hash {
            json!({ "LocationHash": hash })
        } else {
            json!({ "CodeLocation": self.request["Location"]["CodeLocation"] })
        }
    }

    /// A read, status-read or delete request naming this row's identity by `identifier`.
    fn addressed(&self, identifier: Value) -> Value {
        json!({
            "Service": self.request["Service"],
            "Environment": self.request["Environment"],
            "SignalType": self.request["SignalType"],
            "InstrumentationType": self.request["InstrumentationType"],
            "LocationIdentifier": identifier,
        })
    }

    /// The store's scope and key for this row's identity.
    fn stored_at(&self, hash: &str) -> (InstrumentationScope, String) {
        (
            InstrumentationScope::new(
                self.request["InstrumentationType"].as_str().unwrap_or(""),
                self.request["Service"].as_str().unwrap_or(""),
                self.request["Environment"].as_str().unwrap_or(""),
            ),
            instrumentation_key(SNAPSHOT, hash),
        )
    }
}

/// How a location came to hold no configuration.
#[derive(Clone, Copy, Debug)]
enum Gone {
    /// Nothing was ever stored at it.
    NeverStored,
    /// A configuration was stored and then deleted.
    Deleted,
    /// A configuration was stored and its `ExpiresAt` fell due, which only a BREAKPOINT
    /// configuration carries.
    Lapsed,
}

/// Rule 1: an instrumentation-configuration refusal carries `Message` alone, and the read
/// paths distinguish a configuration that is gone from one that never existed while the
/// delete path words both the same way.
///
/// Every row drives all three single-resource operations the model gives
/// `ResourceNotFoundException` to — the configuration read, the status read and the delete —
/// over each of the three ways a location holds nothing.
#[tokio::test]
async fn an_unresolvable_configuration_is_refused_by_message_alone() {
    for case in cases() {
        for gone in [Gone::NeverStored, Gone::Deleted, Gone::Lapsed] {
            if matches!(gone, Gone::Lapsed)
                && case.request["InstrumentationType"] != json!(BREAKPOINT)
            {
                // `ExpiresAt` is a BREAKPOINT configuration's; a PROBE record has none to
                // fall due.
                continue;
            }
            let label = format!("{:?} {}", gone, case.label);
            let (router, engine) = router_with_engine();
            let identifier = stage(&router, &engine, &case, gone, &label).await;

            // A read, whichever of the two reads it is, reports WHICH state the location is
            // in; the delete reports that there is nothing to delete.
            let read_message = match gone {
                Gone::NeverStored => CONFIGURATION_ABSENT,
                Gone::Deleted | Gone::Lapsed => CONFIGURATION_DELETED,
            };
            for (what, uri, expected) in [
                (
                    "GetInstrumentationConfiguration",
                    "/get-instrumentation-configuration",
                    read_message,
                ),
                (
                    "GetInstrumentationConfigurationStatus",
                    "/get-instrumentation-configuration-status",
                    read_message,
                ),
                (
                    "DeleteInstrumentationConfiguration",
                    "/delete-instrumentation-configuration",
                    CONFIGURATION_ABSENT,
                ),
            ] {
                let (status, error) =
                    post(&router, uri, &case.addressed(identifier.clone())).await;
                let case_label = format!("{label} / {what}");
                assert_eq!(
                    status, 404,
                    "case {case_label:?}: expected HTTP 404, got {status} with {error}"
                );
                assert_eq!(
                    error["__type"].as_str(),
                    Some("ResourceNotFoundException"),
                    "case {case_label:?}: expected a ResourceNotFoundException, got {error}"
                );
                let message = error["Message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, expected,
                    "case {case_label:?}: expected {expected:?}, got {message:?}"
                );
                assert!(
                    error["ResourceId"].is_null() && error["ResourceType"].is_null(),
                    "case {case_label:?}: expected the body to carry Message alone, got {error}"
                );
                assert_identifier_free(&case_label, &case, message);
            }
        }
    }
}

/// Bring `case`'s location into the `gone` state and answer the `LocationIdentifier` the
/// refusals are then asked for.
async fn stage(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &Case,
    gone: Gone,
    label: &str,
) -> Value {
    if matches!(gone, Gone::NeverStored) {
        // Nothing is created, so this row's own location names no record — and a row that
        // addresses locations by hash names one that was never minted.
        return case.identifier(UNMINTED_HASH);
    }

    let (status, created) = post(router, "/create-instrumentation-configuration", &case.request)
        .await;
    assert_eq!(
        status, 200,
        "case {label:?}: expected the setup create to succeed, got {status} with {created}"
    );
    let hash = created["LocationHash"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let identifier = case.identifier(&hash);

    match gone {
        Gone::NeverStored => unreachable!("returned above"),
        Gone::Deleted => {
            let (status, deleted) = post(
                router,
                "/delete-instrumentation-configuration",
                &case.addressed(identifier.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {label:?}: expected the setup delete to succeed, got {status} with {deleted}"
            );
        }
        Gone::Lapsed => {
            // The record's own expiry is moved behind the clock, which is the state the
            // clock would have reached on its own after the documented lifetime; the next
            // call materialises the lapse.
            let lapsed_at = DateTime::from_secs_f64(
                DateTime::from(std::time::SystemTime::now())
                    .as_secs_f64()
                    .trunc()
                    - 1.0,
            );
            let (scope, key) = case.stored_at(&hash);
            let state = engine.state::<ComAmazonawsApplicationsignalsApplicationsignalsState>();
            let mut store = state.lock();
            let mut record = store
                .instrumentation_configuration(&scope, &key)
                .expect("the create stored the configuration under its identity")
                .clone();
            record.expires_at = Some(lapsed_at);
            store.put_instrumentation_configuration(scope, record, lapsed_at);
        }
    }
    identifier
}

/// Assert that `message` states no value of the identity the request carried: no ARN, no
/// service or environment name, no signal type and no location hash.
fn assert_identifier_free(label: &str, case: &Case, message: &str) {
    let mut forbidden = vec![
        "arn:".to_string(),
        REGION.to_string(),
        ACCOUNT.to_string(),
        SNAPSHOT.to_string(),
    ];
    for member in ["Service", "Environment", "InstrumentationType"] {
        if let Some(value) = case.request[member].as_str() {
            forbidden.push(value.to_string());
        }
    }
    if let Some(path) = case.request["Location"]["CodeLocation"]["FilePath"].as_str() {
        forbidden.push(path.to_string());
    }
    for value in &forbidden {
        assert!(
            !message.contains(value.as_str()),
            "case {label:?}: the message must not state {value:?}, got {message:?}"
        );
    }
}

/// Rule 2: a second create at one identity is refused with the conflict worded over the four
/// identity dimensions, naming no value of any of them.
///
/// The rows differ along the identity (the table above) and along the rest of the repeated
/// request, which the conflict must not depend on: the other instrumentation type at the same
/// location (the type is not part of the identity), a different description, a different
/// capture configuration and attribute filters the stored record does not carry.
#[tokio::test]
async fn a_repeated_create_conflicts_on_the_identity_dimensions() {
    for case in cases() {
        let (router, _engine) = router_with_engine();
        let (status, created) =
            post(&router, "/create-instrumentation-configuration", &case.request).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the first create to succeed, got {status} with {created}",
            case.label
        );

        let other_type = if case.request["InstrumentationType"] == json!(BREAKPOINT) {
            PROBE
        } else {
            BREAKPOINT
        };
        let mut repeats = vec![("the same request again", case.request.clone())];
        let mut reworded = case.request.clone();
        reworded["Description"] = json!("a different description");
        reworded["AttributeFilters"] = json!([{ "deployment.environment": "prod" }]);
        reworded["CaptureConfiguration"] =
            json!({ "CodeCapture": { "CaptureReturn": true, "CaptureLimits": { "MaxHits": 2 } } });
        repeats.push(("a reworded request at the same location", reworded));
        let mut swapped = case.request.clone();
        swapped["InstrumentationType"] = json!(other_type);
        // `ExpiresAt` is only supported for BREAKPOINT, and the stored record carries the
        // default either way, so nothing but the type changes here.
        repeats.push(("the other instrumentation type", swapped));

        for (why, body) in repeats {
            let (status, error) =
                post(&router, "/create-instrumentation-configuration", &body).await;
            let label = format!("{} / {why}", case.label);
            assert_eq!(
                status, 409,
                "case {label:?}: expected HTTP 409, got {status} with {error}"
            );
            assert_eq!(
                error["__type"].as_str(),
                Some("ConflictException"),
                "case {label:?}: expected a ConflictException, got {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, CONFIGURATION_CONFLICT,
                "case {label:?}: expected {CONFIGURATION_CONFLICT:?}, got {message:?}"
            );
            assert_identifier_free(&label, &case, message);
        }
    }
}

/// Rule 3: `UpdateServiceLevelObjective` refuses an `Id` that names no stored SLO in the
/// work-item layer's wording.
///
/// The rows differ along the name's form (the character classes the modelled
/// `ServiceLevelObjectiveName` pattern `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`
/// admits: plain, dotted, underscored, spaced, and the shortest legal name), the id form the
/// request named the SLO by (bare name or full ARN, both of which the modelled
/// `ServiceLevelObjectiveId` pattern admits), and whether the SLO never existed or was
/// created and then deleted. The discriminator is asserted as a 12-digit server-minted value,
/// never as particular digits: it is the id of a work item, and nothing of the request
/// carries it.
#[tokio::test]
async fn an_unknown_slo_update_is_refused_in_work_item_wording() {
    struct Case {
        name: &'static str,
        created_then_deleted: bool,
        by_arn: bool,
    }

    let cases = [
        Case { name: "absent-slo", created_then_deleted: false, by_arn: false },
        Case { name: "absent-slo", created_then_deleted: false, by_arn: true },
        Case { name: "absent.slo.dotted", created_then_deleted: false, by_arn: true },
        Case { name: "absent_slo_underscored", created_then_deleted: false, by_arn: false },
        Case { name: "absent slo spaced", created_then_deleted: false, by_arn: false },
        Case { name: "9z", created_then_deleted: false, by_arn: true },
        Case { name: "deleted-slo", created_then_deleted: true, by_arn: false },
        Case { name: "deleted-slo-by-arn", created_then_deleted: true, by_arn: true },
    ];

    let mut discriminators: Vec<(String, String)> = Vec::new();
    for case in &cases {
        let router = slo_router();
        if case.created_then_deleted {
            let (status, created) = slo_create(
                &router,
                json!({ "Name": case.name,
                        "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the setup create to succeed, got {status} with {created}",
                case.name
            );
            let (status, deleted) = slo_call(
                &router,
                "DELETE",
                &format!("/slo/{}", name_path_segment(case.name)),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {:?}: expected the setup delete to succeed, got {status} with {deleted}",
                case.name
            );
        }

        let path = if case.by_arn {
            arn_path_segment(case.name)
        } else {
            name_path_segment(case.name)
        };
        let label = format!("{:?} by_arn={}", case.name, case.by_arn);
        let (status, error) = slo_call(
            &router,
            "PATCH",
            &format!("/slo/{path}"),
            Some(json!({ "Description": "a new description" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label}: expected HTTP 404 for an SLO that is not stored, got {status} with {error}"
        );
        let message = error["Message"].as_str().unwrap_or_default();
        let discriminator = assert_work_item_wording(&label, case.name, message);

        // The same work item answers the same discriminator every time: it is the server's id
        // for that item, not a value of this call.
        let (status, again) = slo_call(
            &router,
            "PATCH",
            &format!("/slo/{path}"),
            Some(json!({ "Description": "a third description" })),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {label}: expected the repeat to be refused too, got {status} with {again}"
        );
        assert_eq!(
            again["Message"].as_str(),
            Some(message),
            "case {label}: expected one work item to be reported one way, got {again}"
        );
        discriminators.push((case.name.to_string(), discriminator));
    }

    // Two different work items get two different discriminators.
    for (left_name, left) in &discriminators {
        for (right_name, right) in &discriminators {
            if left_name != right_name {
                assert_ne!(
                    left, right,
                    "case {left_name:?} vs {right_name:?}: expected two work items to carry \
                     two discriminators, got {left:?} for both"
                );
            }
        }
    }
}

/// Assert that `message` is the work-item refusal for the SLO called `name`, and answer the
/// discriminator it carried.
///
/// The fixed text is `Work item with id: ` … ` and namespace: slo does not exist`, with no
/// trailing period; the variable parts are the caller's account, the SLO name taken from the
/// request and the work item's own 12-digit discriminator, in that order and colon-separated.
pub(crate) fn assert_work_item_wording(label: &str, name: &str, message: &str) -> String {
    let prefix = format!("Work item with id: {ACCOUNT}:{name}:");
    let discriminator = message
        .strip_prefix(prefix.as_str())
        .and_then(|rest| rest.strip_suffix(WORK_ITEM_SUFFIX))
        .unwrap_or_else(|| {
            panic!(
                "case {label}: expected a message of the form {prefix:?} + a discriminator + \
                 {WORK_ITEM_SUFFIX:?}, got {message:?}"
            )
        });
    assert_eq!(
        discriminator.len(),
        WORK_ITEM_DISCRIMINATOR_DIGITS,
        "case {label}: expected a {WORK_ITEM_DISCRIMINATOR_DIGITS}-digit work item id, got \
         {discriminator:?}"
    );
    assert!(
        discriminator.bytes().all(|byte| byte.is_ascii_digit()),
        "case {label}: expected the work item id to be digits only, got {discriminator:?}"
    );
    assert!(
        !message.ends_with('.'),
        "case {label}: expected no trailing period, got {message:?}"
    );
    discriminator.to_string()
}
