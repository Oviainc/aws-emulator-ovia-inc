// SPDX-License-Identifier: CC0-1.0

//! AppConfig's AWS-authored extensions, as an account that created none of them sees
//! them.
//!
//! The rule under test: an extension AppConfig itself authors belongs to every account's
//! extension catalogue without that account creating it, so `ListExtensions` reports it
//! beside the account's own extensions and `GetExtension` reports the same record for it
//! by name, by id and by the ARN of its version — while it stays AppConfig's rather than
//! the account's, so no `CreateExtension` may take its name and no `DeleteExtension` may
//! remove it.
//!
//! The table spans the whole published catalogue rather than the one extension a client
//! happened to look for, and every case drives a real HTTP request through the router the
//! emulator's front door uses, so what is asserted is what an AppConfig client sees on the
//! wire. The rows differ along what the rule generalises over: which extension, how many
//! parameters it declares (none and one), which identifier form addresses it (name, id,
//! ARN), whether a version is pinned, and — for the refusals — whether the identifier is a
//! published name or merely shaped like one.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The action-point catalog an assertion checks against is the model's own `ActionPoint`
// enum, re-exported by the service crate as `ACTION_POINT_VALUES`
// (`smithy_query(query="shape",shape="ActionPoint")`), so no action point of a published
// extension can be one AppConfig's workflow has no point for.
use service_com_amazonaws_appconfig_amazonappconfig::ops::types::ACTION_POINT_VALUES;
use service_com_amazonaws_appconfig_amazonappconfig::state::{
    AWS_AUTHORED_EXTENSIONS, AWS_AUTHORED_EXTENSION_VERSION,
};

use crate::appconfig_workflow::{account, router, Answer};
use crate::extensions::{actions, create_extension, get_extension};
use crate::list_extensions::{items, list, walk};
use crate::resource_tags::{arn, pct};

/// The ARN one version of an AWS-authored extension is reported under: the ARN of the
/// same resource shape an account's own extension version carries, with the ACCOUNT field
/// empty, because an authored extension is owned by no account and the model's `Arn`
/// pattern makes that field optional. Built from the suite's own ARN helper, so the
/// partition, the service namespace and the Region are the ones every other AppConfig ARN
/// in these tests carries.
fn authored_arn(extension_id: &str) -> String {
    let owned = arn(&format!(
        "extension/{extension_id}/{AWS_AUTHORED_EXTENSION_VERSION}"
    ));
    owned.replacen(&format!(":{}:", account()), "::", 1)
}

/// What AppConfig's catalogue publishes for the extensions a caller discovers there, as
/// `(id, name)`: the id a caller passes as an `ExtensionIdentifier` and the prose name
/// published beside it.
///
/// Hand-written here rather than read from `crate::state::AWS_AUTHORED_EXTENSIONS`, so a
/// row of that constant that drifted from the published id or the published name fails
/// here instead of agreeing with itself. Source: the catalogue a `ListExtensions` call
/// against AppConfig reports, as the issue this suite grew from quotes it — which is why
/// the entries whose published name it does not quote are pinned by
/// [`PUBLISHED_NOTIFICATION_EXTENSIONS`] instead, beside the parameters they declare.
const PUBLISHED_EXTENSIONS: &[(&str, &str)] = &[
    (
        "AWS.AppConfig.FeatureFlags",
        "AppConfig Feature Flags Helper",
    ),
    (
        "AWS.AppConfig.JiraIntegration",
        "AppConfig integration with Atlassian Jira",
    ),
    (
        "AWS.Evidently.ProjectImporter",
        "Amazon CloudWatch Evidently Project importer",
    ),
];

/// How the published description of the feature-flag helper opens, hand-written from the
/// same source as [`PUBLISHED_EXTENSIONS`]: enough of the sentence to fail a catalogue that
/// describes that extension as something else.
const PUBLISHED_FEATURE_FLAGS_DESCRIPTION_OPENING: &str =
    "Validates AppConfig feature flag data automatically against a JSON schema";

/// One published identifier a caller passes as `ExtensionIdentifier` to associate a
/// deployment-event notification extension, beside the name the catalogue publishes for it
/// and the parameter name that association supplies a value for.
///
/// Hand-written from the AWS AppConfig User Guide's predefined notification extensions —
/// the same source `crate::state::AWS_AUTHORED_EXTENSIONS` is built from — so a catalogue
/// that stopped publishing one of these, or renamed its parameter, fails here rather than
/// only in a replay. The EventBridge row declares no parameter, because that extension
/// publishes to the account's default event bus.
const PUBLISHED_NOTIFICATION_EXTENSIONS: &[(&str, &str, &[&str])] = &[
    (
        "AWS.AppConfig.DeploymentNotificationsToEventBridge",
        "AppConfig deployment events to Amazon EventBridge",
        &[],
    ),
    (
        "AWS.AppConfig.DeploymentEventsToSns",
        "AppConfig deployment events to Amazon SNS",
        &["topicArn"],
    ),
    (
        "AWS.AppConfig.DeploymentEventsToSqs",
        "AppConfig deployment events to Amazon SQS",
        &["queueArn"],
    ),
];

/// `DELETE /extensions/{ExtensionIdentifier}`. The identifier is bound to a NON-greedy
/// `@httpLabel`, so an ARN travels percent-encoded.
async fn delete_extension(router: &MultiServiceRouter, identifier: &str) -> Answer {
    crate::appconfig_workflow::json_call(
        router,
        "DELETE",
        &format!("/extensions/{}", pct(identifier)),
        &Value::Null,
    )
    .await
}

/// The members a reader of an extension must report, as one comparable value: absent and
/// present are both represented, so two answers are compared whole rather than member by
/// member.
fn facts(body: &Value) -> BTreeMap<&'static str, Value> {
    [
        "Id",
        "Name",
        "VersionNumber",
        "Arn",
        "Description",
        "Actions",
        "Parameters",
    ]
    .into_iter()
    .map(|member| (member, body.get(member).cloned().unwrap_or(Value::Null)))
    .collect()
}

/// A 200 answer's body, with the status named in the failure.
fn body_of(case: &str, answer: &Answer) -> Value {
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer.json()
}

/// Every AWS-authored extension is in the listing a fresh account pages through, and
/// `GetExtension` reports the same record for it however it is addressed.
///
/// The row set is the whole published catalogue; within a row the identifier forms (name,
/// id, ARN) and the pinned-version form are the cases.
#[tokio::test]
async fn every_aws_authored_extension_is_listed_and_reads_back_the_same_record() {
    let router = router();
    // Paginated one entry at a time, which is how a client that does not know the
    // catalogue discovers an extension in it.
    let walked = walk(&router, "a fresh account, one entry per page", Some(1)).await;

    for authored in AWS_AUTHORED_EXTENSIONS {
        let case = authored.id;
        let summary = walked
            .iter()
            .find(|item| item.get("Id").and_then(Value::as_str) == Some(authored.id))
            .unwrap_or_else(|| {
                panic!("case {case:?}: the listing must report this extension, got {walked:?}")
            });

        // The catalogue publishes an id a caller types and a name a caller reads, so the
        // summary carries both as published.
        assert_eq!(
            summary.get("Name").and_then(Value::as_str),
            Some(authored.name),
            "case {case:?}: expected the published name, got {summary}"
        );
        assert_eq!(
            summary.get("VersionNumber").and_then(Value::as_i64),
            Some(i64::from(AWS_AUTHORED_EXTENSION_VERSION)),
            "case {case:?}: expected version {AWS_AUTHORED_EXTENSION_VERSION}, got {summary}"
        );
        let expected_arn = authored_arn(authored.id);
        assert_eq!(
            summary.get("Arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the ARN of that version, got {summary}"
        );
        // Field by field, so the one field that makes this ARN an authored extension's is
        // asserted as a rule rather than only as a string: an extension AppConfig authors
        // is owned by NO account, so its ARN names none.
        let fields: Vec<&str> = expected_arn.splitn(6, ':').collect();
        assert_eq!(
            fields.len(),
            6,
            "case {case:?}: an ARN has six fields, got {expected_arn}"
        );
        assert_eq!(
            (fields[0], fields[2], fields[4], fields[5]),
            (
                "arn",
                "appconfig",
                "",
                format!(
                    "extension/{}/{AWS_AUTHORED_EXTENSION_VERSION}",
                    authored.id
                )
                .as_str()
            ),
            "case {case:?}: expected an appconfig ARN owned by no account, got {expected_arn}"
        );
        // A description is reported only when the catalogue publishes one for this
        // extension; `Description` is optional on an `ExtensionSummary`.
        assert_eq!(
            summary.get("Description").and_then(Value::as_str),
            authored.description,
            "case {case:?}: expected the published description, got {summary}"
        );

        // Every identifier form the model's `ExtensionIdentifier` accepts — "the name, the
        // ID, or the Amazon Resource Name (ARN) of the extension" — plus the ARN's own
        // version pinned explicitly, all report ONE record.
        let by_name = body_of(case, &get_extension(&router, authored.name, None).await);
        for (form, identifier, version) in [
            ("by id", authored.id.to_string(), None),
            ("by ARN", expected_arn.clone(), None),
            (
                "by name at its version",
                authored.name.to_string(),
                Some(AWS_AUTHORED_EXTENSION_VERSION),
            ),
            (
                "by ARN at its version",
                expected_arn.clone(),
                Some(AWS_AUTHORED_EXTENSION_VERSION),
            ),
        ] {
            let read = body_of(case, &get_extension(&router, &identifier, version).await);
            assert_eq!(
                facts(&read),
                facts(&by_name),
                "case {case:?} {form}: expected the same record the name reports, got {read}"
            );
        }

        // The summary is that record narrowed to the members `ExtensionSummary` carries.
        for member in ["Id", "Name", "VersionNumber", "Arn", "Description"] {
            assert_eq!(
                summary.get(member).cloned(),
                by_name.get(member).cloned(),
                "case {case:?}: the summary's {member} must be the record's, got {summary}"
            );
        }

        // The actions are the published action points, one action each, each point a value
        // the model's `ActionPoint` enum declares.
        let read_actions = by_name
            .get("Actions")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: expected Actions, got {by_name}"));
        let mut points: Vec<&str> = read_actions.keys().map(String::as_str).collect();
        points.sort_unstable();
        let mut published: Vec<&str> = authored.action_points.to_vec();
        published.sort_unstable();
        assert_eq!(
            points, published,
            "case {case:?}: expected the published action points, got {read_actions:?}"
        );
        for (point, bound) in read_actions {
            assert!(
                ACTION_POINT_VALUES.contains(&point.as_str()),
                "case {case:?}: {point} is not a modelled ActionPoint"
            );
            let bound = bound.as_array().unwrap_or_else(|| {
                panic!("case {case:?}: expected an action list at {point}, got {bound}")
            });
            assert_eq!(
                bound.len(),
                1,
                "case {case:?}: ActionList is bounded len 1..1, got {bound:?}"
            );
            assert_eq!(
                bound[0].get("Name").and_then(Value::as_str),
                Some(point.as_str()),
                "case {case:?}: expected the action to name its point, got {}",
                bound[0]
            );
        }

        // A parameter map is reported only when the extension declares one — `ParameterMap`
        // is bounded `len 1..10`, so an empty map is not a value a client may be handed —
        // and each parameter carries the published name and flags.
        let read_parameters = by_name.get("Parameters").cloned().unwrap_or(Value::Null);
        if authored.parameters.is_empty() {
            assert_eq!(
                read_parameters,
                Value::Null,
                "case {case:?}: an extension declaring no parameter reports none, got \
                 {read_parameters}"
            );
        } else {
            let read_parameters = read_parameters
                .as_object()
                .cloned()
                .unwrap_or_else(|| panic!("case {case:?}: expected Parameters, got {by_name}"));
            assert_eq!(
                read_parameters.len(),
                authored.parameters.len(),
                "case {case:?}: expected {} parameters, got {read_parameters:?}",
                authored.parameters.len()
            );
            for parameter in authored.parameters {
                let read = read_parameters.get(parameter.name).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected the parameter {:?}, got {read_parameters:?}",
                        parameter.name
                    )
                });
                assert_eq!(
                    read.get("Required").and_then(Value::as_bool),
                    Some(parameter.required),
                    "case {case:?}: expected Required {}, got {read}",
                    parameter.required
                );
                // The value is supplied when the extension is associated, which is what a
                // parameter that is not `Dynamic` means.
                assert_eq!(
                    read.get("Dynamic").and_then(Value::as_bool),
                    Some(false),
                    "case {case:?}: expected Dynamic false, got {read}"
                );
            }
        }

        // The listing's `Name` filter narrows to this one extension, so a client can look
        // it up without walking the catalogue.
        let filtered = items(case, &list(&router, &format!("name={}", pct(authored.name))).await);
        assert_eq!(
            filtered.len(),
            1,
            "case {case:?}: a Name filter reports exactly the extension it names, got \
             {filtered:?}"
        );

        // Only the version it is published at exists.
        let missing = get_extension(
            &router,
            authored.id,
            Some(AWS_AUTHORED_EXTENSION_VERSION + 1),
        )
        .await;
        assert_eq!(
            missing.status,
            404,
            "case {case:?}: a version it does not hold is a 404, got {} / {}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
    }
}

/// The catalogue publishes each extension under its own id AND its own prose name, and a
/// caller who knows either reads the same record — asserted against values written here
/// rather than against the service's own catalogue constant.
///
/// Rows: every published entry whose name this suite pins ([`PUBLISHED_EXTENSIONS`]), which
/// differ along whether the name resembles the id at all, whether the entry carries a
/// description, and which product the entry integrates.
#[tokio::test]
async fn the_published_entries_are_listed_under_their_published_ids_and_names() {
    let router = router();
    let walked = walk(&router, "the published catalogue", None).await;

    for (id, published_name) in PUBLISHED_EXTENSIONS {
        let summary = walked
            .iter()
            .find(|item| item.get("Id").and_then(Value::as_str) == Some(*id))
            .unwrap_or_else(|| {
                panic!("case {id:?}: the listing must report this extension, got {walked:?}")
            });
        assert_eq!(
            summary.get("Name").and_then(Value::as_str),
            Some(*published_name),
            "case {id:?}: expected the published name, got {summary}"
        );
        assert_eq!(
            summary.get("VersionNumber").and_then(Value::as_i64),
            Some(i64::from(AWS_AUTHORED_EXTENSION_VERSION)),
            "case {id:?}: expected the published version, got {summary}"
        );
        assert_eq!(
            summary.get("Arn").and_then(Value::as_str),
            Some(authored_arn(id).as_str()),
            "case {id:?}: expected the ARN of that version, owned by no account, got {summary}"
        );

        // The id and the name are BOTH identifiers of the same record: a reader handed
        // either reports the other.
        let by_id = body_of(id, &get_extension(&router, id, None).await);
        let by_name = body_of(id, &get_extension(&router, published_name, None).await);
        assert_eq!(
            facts(&by_id),
            facts(&by_name),
            "case {id:?}: the published name must read the record the id reads, got {by_name}"
        );
        assert_eq!(
            by_id.get("Id").and_then(Value::as_str),
            Some(*id),
            "case {id:?}: expected the published id, got {by_id}"
        );
    }

    // The one published description this suite pins: an entry described as something else
    // fails here rather than only in a replay.
    let helper = body_of(
        "AWS.AppConfig.FeatureFlags",
        &get_extension(&router, "AWS.AppConfig.FeatureFlags", None).await,
    );
    let described = helper
        .get("Description")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        described.starts_with(PUBLISHED_FEATURE_FLAGS_DESCRIPTION_OPENING),
        "case \"AWS.AppConfig.FeatureFlags\": expected a description opening \
         {PUBLISHED_FEATURE_FLAGS_DESCRIPTION_OPENING:?}, got {described:?}"
    );
}

/// The deployment-event notification extensions the AppConfig User Guide publishes are in
/// the catalogue under the identifiers it tells a caller to pass, each declaring the
/// parameter its association supplies.
///
/// Rows: all three published notification extensions, including the one that declares no
/// parameter at all.
#[tokio::test]
async fn the_published_notification_extensions_resolve_under_their_documented_identifiers() {
    let router = router();
    for (identifier, published_name, parameters) in PUBLISHED_NOTIFICATION_EXTENSIONS {
        let read = body_of(identifier, &get_extension(&router, identifier, None).await);
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(*identifier),
            "case {identifier:?}: expected the published identifier as the Id, got {read}"
        );
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(*published_name),
            "case {identifier:?}: expected the published name, got {read}"
        );
        let declared = read
            .get("Parameters")
            .and_then(Value::as_object)
            .map(|map| {
                let mut names: Vec<&str> = map.keys().map(String::as_str).collect();
                names.sort_unstable();
                names
            })
            .unwrap_or_default();
        let mut expected: Vec<&str> = parameters.to_vec();
        expected.sort_unstable();
        assert_eq!(
            declared, expected,
            "case {identifier:?}: expected the published parameter names, got {read}"
        );
    }
}

/// An AWS-authored extension is AppConfig's, not the account's: its name cannot be created
/// and the extension cannot be deleted, and neither refusal changes the catalogue.
///
/// Rows differ along which extension, which identifier form the delete names it by, and —
/// for the create — whether the name is a published one or merely shaped like one.
#[tokio::test]
async fn an_aws_authored_extension_is_not_this_accounts_to_create_or_delete() {
    let router = router();
    let before = walk(&router, "before the refusals", None).await;

    for authored in AWS_AUTHORED_EXTENSIONS {
        let case = authored.id;
        let identifiers = [
            ("by id", authored.id.to_string()),
            ("by name", authored.name.to_string()),
            ("by ARN", authored_arn(authored.id)),
        ];
        for (form, identifier) in identifiers {
            let answer = delete_extension(&router, &identifier).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?} {form}: expected 400 for a delete of an authored extension, \
                 got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert_eq!(
                answer
                    .error_code()
                    .split('#')
                    .next_back()
                    .unwrap_or_default(),
                "BadRequestException",
                "case {case:?} {form}: expected BadRequestException, got {}",
                answer.error_code()
            );
        }

        // Both published forms are taken in every account, so neither is a name a create
        // may take over.
        for (form, taken) in [
            ("its published name", authored.name),
            ("its published id", authored.id),
        ] {
            let created = create_extension(
                &router,
                &json!({
                    "Name": taken,
                    "Actions": actions(&[ACTION_POINT_VALUES[0]]),
                }),
                None,
            )
            .await;
            assert_eq!(
                created.status,
                400,
                "case {case:?} {form}: expected 400 for a create under an authored name, got {} \
                 / {}",
                created.status,
                String::from_utf8_lossy(&created.body)
            );
        }

        // The record a reader answers for it is still the published one, so neither refusal
        // left a half-written extension behind.
        let read = body_of(case, &get_extension(&router, authored.id, None).await);
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            authored.description,
            "case {case:?}: expected the published record after the refusals, got {read}"
        );
    }

    // A name merely SHAPED like a published one is the account's to create: the refusal is
    // equality on the published name, not a prefix match.
    for suffix in ["-mine", ".Extra", " copy"] {
        let name = format!("{}{suffix}", AWS_AUTHORED_EXTENSIONS[0].name);
        let created = create_extension(
            &router,
            &json!({
                "Name": name,
                "Actions": actions(&[ACTION_POINT_VALUES[0]]),
            }),
            None,
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {name:?}: a name no authored extension carries is the account's to create, \
             got {} / {}",
            created.status,
            String::from_utf8_lossy(&created.body)
        );
        let deleted = delete_extension(&router, &name).await;
        assert_eq!(
            deleted.status,
            204,
            "case {name:?}: an extension this account created is its to delete, got {} / {}",
            deleted.status,
            String::from_utf8_lossy(&deleted.body)
        );
    }

    // The catalogue is exactly what it was before every refusal above.
    let after = walk(&router, "after the refusals", None).await;
    assert_eq!(
        after, before,
        "the refused calls must leave the catalogue unchanged"
    );
}
