// SPDX-License-Identifier: CC0-1.0

//! The PROSE this service refuses with, per error class, over HTTP.
//!
//! The rule under test: a refusal carries the sentence its error class is worded with — the
//! fixed text exactly, and as variable parts only what that wording names. So:
//!
//! * a create that defines no service level indicator states the PAIR of members one of which
//!   must be populated, and nothing of the rest of the request;
//! * an update that would change a stored SLO's evaluation kind states the DIRECTION it
//!   attempted, read off the stored record's kind and the kind of the supplied config;
//! * a create whose identifier is already taken states that generically, naming neither the
//!   resource, its name nor its ARN — the identifier at fault is the one the caller sent;
//! * the TAGGING plane, which addresses every kind of resource by `ResourceArn` alone, names
//!   the resource generically and echoes the ARN the request carried, whichever kind that ARN
//!   addresses;
//! * a batch delete's per-item failure states that the configuration is gone, naming no
//!   identifier: the entry's own `ResourceArn` is which item failed.
//!
//! Every rule is asserted over a TABLE spanning what it generalises over, and each table also
//! drives the ADMITTED side of its rule, so the wording is pinned about the requests that are
//! refused rather than about every request to these operations.
//!
//! The constants and builders below are this test target's one spelling of these sentences;
//! the sibling suites that meet one of them import it from here.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{list_tags_raw, slo_arn, tag, untag};
use crate::slo_evaluation_kind::{
    request_based_sli_config, request_based_sli_config_with_queries, sli_config_with_queries,
};
use crate::slo_lifecycle::{
    arn_uri_segment, call, create, router, sli_config, ACCOUNT, REGION,
};
use crate::tag_attachment::tag_resource;

/// The sentence a create that populates neither SLI config member is refused with. The two
/// member names are the ones the model declares
/// (`smithy_query(query="operation",operation="CreateServiceLevelObjective")`: `SliConfig`,
/// `RequestBasedSliConfig`), and there is no trailing period.
pub(crate) const MISSING_SLI_CONFIG: &str =
    "Either SliConfig or RequestBasedSliConfig must be populated";

/// The sentence a create whose `Name` is already stored is refused with: generic, with no
/// trailing period and naming no identifier.
pub(crate) const IDENTIFIER_TAKEN: &str = "A resource with the same identifier already exists";

/// The sentence a batch delete's per-item failure states for an ARN that names no stored
/// configuration. One sentence for both ways a configuration goes, which this path does not
/// distinguish.
pub(crate) const CONFIGURATION_GONE: &str =
    "The instrumentation config is already deleted or expired.";

/// How prose names the evaluation kind of an SLO that is evaluated over periods, as the SLO
/// operations' own docs word it ("a period-based SLO"). Not the `EvaluationType` token a
/// response reports (`PeriodBased`).
pub(crate) const PERIOD_BASED: &str = "period-based";

/// [`PERIOD_BASED`], for the kind evaluated over requests.
pub(crate) const REQUEST_BASED: &str = "request-based";

/// The sentence an update that would change a stored SLO's evaluation kind from `stored` to
/// `requested` is refused with: fixed text around the two kinds, in that order, with no
/// trailing period.
pub(crate) fn kind_switch_refusal(stored: &str, requested: &str) -> String {
    format!("Updating from {stored} SLO to {requested} SLO is not allowed")
}

/// The sentence a tagging operation refuses a `ResourceArn` naming no stored resource with:
/// the generic noun and the ARN the request carried, verbatim.
pub(crate) fn tagged_resource_missing(resource_arn: &str) -> String {
    format!("Resource with arn '{resource_arn}' does not exist.")
}

/// The capitalised type names this service reports in `ResourceType`, which the tagging
/// plane's MESSAGE must not spell: a message that named the kind would differ between two
/// calls the operation treats identically.
const RESOURCE_TYPE_WORDS: [&str; 2] = ["Slo", "InstrumentationConfiguration"];

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
const UNMINTED_HASH: &str = "0a1b2c3d4e5f6789";

/// `POST <uri>` with `body`.
async fn post(router: &MultiServiceRouter, uri: &str, body: Value) -> (u16, Value) {
    call(router, "POST", uri, Some(body)).await
}

/// The `message` a `ValidationException` body carries (the modelled member is lower case on
/// this shape), else the empty string.
fn validation_message(body: &Value) -> &str {
    body["message"].as_str().unwrap_or_default()
}

/// Assert that `message` is a bare sentence rather than a constraint-violation envelope: the
/// rules below hold of no single member, so the envelope the boundary validator renders for a
/// modelled constraint would report a member the request need not have sent.
fn assert_not_an_envelope(label: &str, message: &str) {
    for fragment in ["validation error", "Value at '", "failed to satisfy constraint"] {
        assert!(
            !message.contains(fragment),
            "case {label:?}: expected a bare sentence, but it carries {fragment:?}: {message:?}"
        );
    }
}

/// A create that populates NEITHER SLI config is refused 400 `ValidationException` with the
/// one sentence naming the pair, whatever else the request carries, and stores nothing.
///
/// The rows differ along what the rule generalises over: how much of the rest of the input the
/// request fills (nothing beyond the name, each optional member, all of them), since the rule
/// is about the absent pair and nothing else.
#[tokio::test]
async fn a_create_with_no_indicator_states_the_pair_it_must_populate() {
    let rows: Vec<(&str, Value)> = vec![
        ("the name alone", json!({})),
        (
            "a description only",
            json!({ "Description": "an objective with no indicator" }),
        ),
        (
            "a goal only",
            json!({ "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 30.0 } }),
        ),
        (
            "burn rate configurations only",
            json!({ "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }] }),
        ),
        (
            "tags only",
            json!({ "Tags": [{ "Key": "team", "Value": "checkout" }] }),
        ),
        (
            "the recommended-SLO flag, which supplies no config of its own",
            json!({ "CreateRecommendedSlo": true }),
        ),
        (
            "every optional member but the two configs",
            json!({
                "Description": "still no indicator",
                "Goal": { "AttainmentGoal": 95.0 },
                "BurnRateConfigurations": [{ "LookBackWindowMinutes": 30 }],
                "Tags": [{ "Key": "team", "Value": "orders" }],
                "CreateRecommendedSlo": true,
                "AutoInvestigationEnabled": true,
            }),
        ),
    ];

    for (index, (label, extra)) in rows.iter().enumerate() {
        let router = router();
        let name = format!("no-indicator-{index:02}");
        let mut body = extra.clone();
        body["Name"] = json!(name);

        let (status, answer) = create(&router, body).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a create with no indicator, got {status} \
             with {answer}"
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {answer}"
        );
        let message = validation_message(&answer);
        assert_eq!(
            message, MISSING_SLI_CONFIG,
            "case {label:?}: expected {MISSING_SLI_CONFIG:?}, got {message:?}"
        );
        assert_not_an_envelope(label, message);

        // Nothing was stored, so the sentence is about a refused create rather than a
        // half-written record.
        let (status, after) = call(&router, "GET", &format!("/slo/{name}"), None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected a refused create to store nothing, got {status} with \
             {after}"
        );
    }

    // The admitted side: a create that populates EITHER member is served.
    for (label, member, config) in [
        ("the period-based member", "SliConfig", sli_config("checkout", "GET /cart", 100.0)),
        (
            "the request-based member",
            "RequestBasedSliConfig",
            request_based_sli_config("checkout"),
        ),
    ] {
        let router = router();
        let (status, created) =
            create(&router, json!({ "Name": "with-indicator", member: config })).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a create that populates one member to be served, got \
             {status} with {created}"
        );
    }
}

/// An update whose SLI config is of the other kind than the stored record is refused 400
/// `ValidationException` naming the DIRECTION attempted — the stored kind, then the kind the
/// supplied config would make it — and writes nothing.
///
/// The rows differ along the direction (both ways), the FORM of the stored config and of the
/// offending one (an Application Signals entity, the caller's own metric queries) and which
/// other members ride along in the refused request.
#[tokio::test]
async fn an_update_that_switches_the_evaluation_kind_states_the_direction() {
    struct Row {
        label: &'static str,
        /// The member the setup create supplies, and its config.
        created_with: (&'static str, Value),
        /// The patch that tries to switch the kind.
        patch: Value,
        /// The kind the stored record is of, and the kind the patch would make it.
        direction: (&'static str, &'static str),
    }

    let rows = [
        Row {
            label: "period-based record, request-based entity config",
            created_with: ("SliConfig", sli_config("checkout", "GET /cart", 100.0)),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
            direction: (PERIOD_BASED, REQUEST_BASED),
        },
        Row {
            label: "period-based query record, request-based query config",
            created_with: ("SliConfig", sli_config_with_queries()),
            patch: json!({ "RequestBasedSliConfig": request_based_sli_config_with_queries() }),
            direction: (PERIOD_BASED, REQUEST_BASED),
        },
        Row {
            label: "request-based record, period-based entity config",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("orders")),
            patch: json!({ "SliConfig": sli_config("orders", "POST /orders", 250.0) }),
            direction: (REQUEST_BASED, PERIOD_BASED),
        },
        Row {
            label: "request-based query record, period-based query config",
            created_with: (
                "RequestBasedSliConfig",
                request_based_sli_config_with_queries(),
            ),
            patch: json!({ "SliConfig": sli_config_with_queries() }),
            direction: (REQUEST_BASED, PERIOD_BASED),
        },
        Row {
            label: "request-based record, the switch bundled with other members",
            created_with: ("RequestBasedSliConfig", request_based_sli_config("payments")),
            patch: json!({
                "SliConfig": sli_config("payments", "GET /pay", 10.0),
                "Description": "after",
                "Goal": { "AttainmentGoal": 90.0 },
                "AutoInvestigationEnabled": true,
            }),
            direction: (REQUEST_BASED, PERIOD_BASED),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let router = router();
        let name = format!("fixed-kind-{index:02}");
        let mut setup = json!({ "Name": name, "Description": "before" });
        setup[row.created_with.0] = row.created_with.1.clone();
        let (status, created) = create(&router, setup).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the setup create to succeed, got {status} with {created}",
            row.label
        );
        let before = created["Slo"].clone();

        let (status, answer) =
            call(&router, "PATCH", &format!("/slo/{name}"), Some(row.patch.clone())).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400 for a kind switch, got {status} with {answer}",
            row.label
        );
        assert_eq!(
            answer["__type"].as_str(),
            Some("ValidationException"),
            "case {:?}: expected a ValidationException, got {answer}",
            row.label
        );
        let expected = kind_switch_refusal(row.direction.0, row.direction.1);
        let message = validation_message(&answer);
        assert_eq!(
            message, expected,
            "case {:?}: expected {expected:?}, got {message:?}",
            row.label
        );
        assert_not_an_envelope(row.label, message);
        // The direction is named, so the mirror sentence is NOT what the client reads.
        let mirror = kind_switch_refusal(row.direction.1, row.direction.0);
        assert_ne!(
            message, mirror,
            "case {:?}: expected the attempted direction, not its mirror {mirror:?}",
            row.label
        );

        // Round trip: the refused update wrote nothing, so the record still reads as it was.
        let (status, after) = call(&router, "GET", &format!("/slo/{name}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the stored SLO to still be readable, got {status} with {after}",
            row.label
        );
        assert_eq!(
            after["Slo"], before,
            "case {:?}: expected a refused update to leave the record untouched, got {:?}",
            row.label, after["Slo"]
        );
    }
}

/// A create under a `Name` already stored is refused 409 `ConflictException` with the one
/// generic sentence, which identifies nothing.
///
/// The rows differ along the name's form — the character classes the modelled
/// `ServiceLevelObjectiveName` pattern `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$` admits
/// — including the shortest name it admits and one of its maximum length.
#[tokio::test]
async fn a_create_whose_identifier_is_taken_identifies_nothing() {
    let longest = format!("a{}z", "b".repeat(126));
    let names = vec![
        "taken-slo".to_string(),
        "taken.slo.dotted".to_string(),
        "taken_slo_underscored".to_string(),
        "taken slo spaced".to_string(),
        "9z".to_string(),
        longest,
    ];

    for name in &names {
        let router = router();
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the first create to succeed, got {status} with {created}"
        );
        let stored_arn = created["Slo"]["Arn"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        for (why, body) in [
            (
                "the same request again",
                json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
            ),
            (
                "a request-based config under the same name",
                json!({ "Name": name, "RequestBasedSliConfig": request_based_sli_config("x") }),
            ),
        ] {
            let (status, answer) = create(&router, body).await;
            let label = format!("{name:?} / {why}");
            assert_eq!(
                status, 409,
                "case {label}: expected HTTP 409 for a taken identifier, got {status} with \
                 {answer}"
            );
            assert_eq!(
                answer["__type"].as_str(),
                Some("ConflictException"),
                "case {label}: expected a ConflictException, got {answer}"
            );
            let message = answer["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, IDENTIFIER_TAKEN,
                "case {label}: expected {IDENTIFIER_TAKEN:?}, got {message:?}"
            );
            for identifier in [name.as_str(), stored_arn.as_str(), "arn:"] {
                assert!(
                    !message.contains(identifier),
                    "case {label}: the conflict must identify nothing, but it states \
                     {identifier:?}: {message:?}"
                );
            }
        }
    }

    // The admitted side: another name is still created, so the conflict is about the
    // identifier and not about the request.
    let router = router();
    for name in &names {
        let (status, created) = create(
            &router,
            json!({ "Name": name, "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected distinct names to be served on one store, got {status} \
             with {created}"
        );
    }
}

/// Every tagging operation refuses a `ResourceArn` that names no stored resource with the
/// plane's own generic sentence: the generic noun and the ARN the request carried, and no
/// concrete resource type.
///
/// The rows differ along WHICH kind of ARN misses (an SLO of this account, an instrumentation
/// configuration of this account, another account's resource, another service's resource, and
/// a bare name, which is no ARN at all) and along WHICH tagging operation refuses it — the
/// write, the removal and the read, all three of which resolve the ARN the same way.
#[tokio::test]
async fn a_tagging_refusal_names_the_resource_generically() {
    let configuration_arn = format!(
        "arn:aws:application-signals:{REGION}:{ACCOUNT}:instrumentationConfig/checkout/prod/\
         {SNAPSHOT}/{UNMINTED_HASH}"
    );
    let rows: Vec<(&str, String)> = vec![
        ("an SLO that was never created", slo_arn("absent-slo")),
        (
            "an SLO name with a legal but unused spelling",
            slo_arn("absent.slo.dotted"),
        ),
        (
            "a configuration location nothing was created at",
            configuration_arn,
        ),
        (
            "another account's SLO",
            slo_arn("absent-slo").replace(ACCOUNT, "111111111111"),
        ),
        (
            "another service's resource",
            format!("arn:aws:cloudwatch:{REGION}:{ACCOUNT}:alarm/my-alarm"),
        ),
        ("a bare resource name, not an ARN", "absent-slo".to_string()),
    ];

    for (label, resource_arn) in &rows {
        let router = router();
        // A live SLO, so the store is not empty and the refusal is about the ARN named.
        let (status, created) = create(
            &router,
            json!({ "Name": "live-slo", "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected the setup create to succeed, got {status} with {created}"
        );

        let keys = vec!["team".to_string()];
        let answers = vec![
            (
                "TagResource",
                tag_resource(&router, resource_arn, &[tag("team", "checkout")]).await,
            ),
            ("UntagResource", untag(&router, resource_arn, &keys).await),
            (
                "ListTagsForResource",
                list_tags_raw(&router, &arn_uri_segment(resource_arn)).await,
            ),
        ];

        for (what, (status, error)) in answers {
            let case = format!("{label:?} / {what}");
            assert_eq!(
                status, 404,
                "case {case}: expected HTTP 404, got {status} with {error}"
            );
            assert_eq!(
                error["__type"].as_str(),
                Some("ResourceNotFoundException"),
                "case {case}: expected a ResourceNotFoundException, got {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            let expected = tagged_resource_missing(resource_arn);
            assert_eq!(
                message, expected,
                "case {case}: expected {expected:?}, got {message:?}"
            );
            for word in RESOURCE_TYPE_WORDS {
                assert!(
                    !message.contains(word),
                    "case {case}: the tagging plane's message names no concrete type, but it \
                     states {word:?}: {message:?}"
                );
            }
        }

        // The admitted side: the live resource is still tagged and read through the same
        // operations, so the wording above is about the ARN that resolves nothing.
        let live_arn = slo_arn("live-slo");
        let (status, tagged) = tag_resource(&router, &live_arn, &[tag("team", "checkout")]).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a live ARN to be tagged, got {status} with {tagged}"
        );
        let (status, listed) = list_tags_raw(&router, &arn_uri_segment(&live_arn)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected a live ARN's tags to be readable, got {status} with \
             {listed}"
        );
    }
}

/// `BatchDeleteInstrumentationConfigurations` reports an ARN that names no stored
/// configuration as a per-ITEM failure stating that the configuration is gone, with no
/// identifier in the sentence — the entry's `ResourceArn` is which item failed.
///
/// The rows differ along the instrumentation type the target carries and along WHY an ARN
/// resolves nothing: a location-hash segment that was never minted, an ARN of another account,
/// a configuration this very request has already deleted, and an ARN of this service's other
/// resource kind.
#[tokio::test]
async fn a_batch_delete_reports_a_missing_item_as_already_gone() {
    for instrumentation_type in [PROBE, BREAKPOINT] {
        let router = router();
        let service = "checkout";
        let environment = "prod";
        let (status, created) = post(
            &router,
            "/create-instrumentation-configuration",
            json!({
                "InstrumentationType": instrumentation_type,
                "Service": service,
                "Environment": environment,
                "SignalType": SNAPSHOT,
                "Location": { "CodeLocation": {
                    "Language": "Java",
                    "ClassName": "com.example.CartService",
                    "FilePath": "src/main/java/com/example/CartService.java",
                    "LineNumber": 42,
                }},
                "CaptureConfiguration": { "CodeCapture": { "CaptureLimits": { "MaxHits": 3 } } },
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {instrumentation_type:?}: expected the setup create to succeed, got {status} \
             with {created}"
        );
        let live_arn = created["ARN"].as_str().unwrap_or_default().to_string();
        let hash = created["LocationHash"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        // Each ARN below names nothing, for a different reason; `live_arn` is the one item
        // the call resolves, listed first so the repeat of it fails as already deleted.
        let unminted = live_arn.replace(&hash, UNMINTED_HASH);
        let other_account = live_arn.replace(ACCOUNT, "111111111111");
        let other_kind = slo_arn("absent-slo");
        let listed = vec![
            live_arn.clone(),
            unminted.clone(),
            other_account.clone(),
            live_arn.clone(),
            other_kind.clone(),
        ];

        let (status, answer) = post(
            &router,
            "/batch-delete-instrumentation-configurations",
            json!({
                "DeletionTarget": {
                    "ResourceArns": {
                        "ResourceArns": listed,
                        "InstrumentationType": instrumentation_type,
                    }
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {instrumentation_type:?}: expected HTTP 200 from the batch delete, got \
             {status} with {answer}"
        );
        assert_eq!(
            answer["DeletedCount"].as_i64(),
            Some(1),
            "case {instrumentation_type:?}: expected the one resolved ARN to be deleted, got \
             {answer}"
        );

        let errors = answer["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors
                .iter()
                .map(|error| error["ResourceArn"].as_str().unwrap_or_default().to_string())
                .collect::<Vec<String>>(),
            vec![unminted, other_account, live_arn, other_kind],
            "case {instrumentation_type:?}: expected every ARN naming nothing to fail per \
             item, got {answer}"
        );
        for error in &errors {
            let case = format!(
                "{instrumentation_type:?} / {}",
                error["ResourceArn"].as_str().unwrap_or_default()
            );
            assert_eq!(
                error["Code"].as_str(),
                Some("ResourceNotFoundException"),
                "case {case}: expected the per-item not-found code, got {error}"
            );
            let message = error["Message"].as_str().unwrap_or_default();
            assert_eq!(
                message, CONFIGURATION_GONE,
                "case {case}: expected {CONFIGURATION_GONE:?}, got {message:?}"
            );
            for value in ["arn:", service, environment, SNAPSHOT, hash.as_str()] {
                assert!(
                    !message.contains(value),
                    "case {case}: the per-item sentence names no identifier, but it states \
                     {value:?}: {message:?}"
                );
            }
        }
    }
}
