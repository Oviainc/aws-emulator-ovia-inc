// SPDX-License-Identifier: CC0-1.0

//! `UpdateGroup`, end to end over the wire.
//!
//! Every case drives the same front door the production router does (the
//! `Xray` harness in [`super::xray_wire`]), so the assertions below are about
//! HTTP responses, not about internal state.
//!
//! The rule under test: `UpdateGroup` selects a stored group by either
//! `GroupName` or `GroupARN`, each member the request SUPPLIES replaces the
//! stored one, each member it OMITS keeps its stored value, and the group's
//! identity — its case-sensitive name and the ARN that was generated upon
//! creation — is untouched; the updated record is then what every later read
//! answers, and a request naming no group is refused with
//! `InvalidRequestException` rather than creating one (a request violating a
//! MODELLED constraint is refused one layer earlier, as `ValidationException`).
//!
//! The rows differ along what that rule generalises over: which members the
//! update supplies (filter only, insights only, both, neither), which selector
//! picks the group, what the group held before the update (an insights
//! configuration or not, which filter expression), the form and length of the
//! supplied values, and both published `GroupName` length bounds. Every group is
//! created WITH a filter expression, because X-Ray refuses a `CreateGroup` that
//! supplies none — see `group_control_plane.rs` for that rule.

use serde_json::{json, Value};

use super::trace_ingest_and_groups::assert_group_arn_shape;
use super::xray_wire::Xray;

/// A group's configurable state: the two members `UpdateGroup` can change.
/// `None` means "not present".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Configuration {
    filter: Option<&'static str>,
    /// `(InsightsEnabled, NotificationsEnabled)`.
    insights: Option<(bool, bool)>,
}

/// One update case: how the group is created, what the update supplies, and
/// which selector names it.
#[derive(Debug, Clone, Copy)]
struct UpdateCase {
    case: &'static str,
    name: &'static str,
    created: Configuration,
    /// The members the `UpdateGroup` request carries.
    update: Configuration,
    /// Select the group by its minted `GroupARN` rather than by `GroupName`.
    by_arn: bool,
}

impl UpdateCase {
    /// What the group must hold after the update: a supplied member replaces,
    /// an omitted one is kept.
    fn expected(&self) -> Configuration {
        Configuration {
            filter: self.update.filter.or(self.created.filter),
            insights: self.update.insights.or(self.created.insights),
        }
    }
}

/// The table. `FilterExpression` carries no modelled constraint, so its rows
/// span the forms X-Ray's filter-expression documentation shows (a keyword
/// comparison, a function call, an annotation predicate, a long conjunction);
/// `GroupName`'s published `@length` is 1..32
/// (`smithy_query(query="shape",shape="UpdateGroupRequest")`), so one row sits
/// at each bound.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "a filter expression onto a group with no insights",
            name: "adopts-filter",
            created: Configuration {
                filter: Some("ok"),
                insights: None,
            },
            update: Configuration {
                filter: Some("fault = true"),
                insights: None,
            },
            by_arn: false,
        },
        UpdateCase {
            case: "a filter expression replaced, selected by ARN",
            name: "replaces-filter",
            created: Configuration {
                filter: Some("fault = true"),
                insights: None,
            },
            update: Configuration {
                filter: Some("service(\"api\") { fault = true OR error = true }"),
                insights: None,
            },
            by_arn: true,
        },
        UpdateCase {
            case: "insights enabled, the stored filter expression untouched",
            name: "adopts-insights",
            created: Configuration {
                filter: Some("responsetime > 2"),
                insights: None,
            },
            update: Configuration {
                filter: None,
                insights: Some((true, false)),
            },
            by_arn: false,
        },
        UpdateCase {
            case: "insights notifications turned on, selected by ARN",
            name: "enables-notifications",
            created: Configuration {
                filter: Some("http.status = 429"),
                insights: Some((true, false)),
            },
            update: Configuration {
                filter: None,
                insights: Some((true, true)),
            },
            by_arn: true,
        },
        UpdateCase {
            case: "insights turned back off",
            name: "disables-insights",
            created: Configuration {
                filter: Some("duration > 5"),
                insights: Some((true, true)),
            },
            update: Configuration {
                filter: None,
                insights: Some((false, false)),
            },
            by_arn: false,
        },
        UpdateCase {
            case: "both members at once",
            name: "replaces-both",
            created: Configuration {
                filter: Some("http.status = 500"),
                insights: Some((false, false)),
            },
            update: Configuration {
                filter: Some("annotation.stage = \"prod\" AND responsetime > 1"),
                insights: Some((true, true)),
            },
            by_arn: true,
        },
        UpdateCase {
            case: "neither member: an update that supplies nothing changes nothing",
            name: "supplies-nothing",
            created: Configuration {
                filter: Some("ok = true"),
                insights: Some((true, false)),
            },
            update: Configuration {
                filter: None,
                insights: None,
            },
            by_arn: false,
        },
        UpdateCase {
            // GroupName's published @length minimum is 1 ...
            case: "the shortest name the model allows",
            name: "a",
            created: Configuration {
                filter: Some("partial"),
                insights: None,
            },
            update: Configuration {
                filter: Some("error = true"),
                insights: Some((true, false)),
            },
            by_arn: false,
        },
        UpdateCase {
            // ... and its maximum is 32.
            case: "the longest name the model allows",
            name: "abcdefghijklmnopqrstuvwxyz012345",
            created: Configuration {
                filter: Some("fault = true"),
                insights: Some((false, false)),
            },
            update: Configuration {
                filter: Some(
                    "service(\"gateway\") AND annotation.tenant = \"acme\" AND responsetime > 10 AND fault = true",
                ),
                insights: None,
            },
            by_arn: true,
        },
    ]
}

/// The `CreateGroup` body for a case's starting configuration.
fn create_body(case: &UpdateCase) -> Value {
    let mut body = json!({ "GroupName": case.name });
    apply_configuration(&mut body, case.created);
    body
}

/// The `UpdateGroup` body for a case: the selector it uses plus the members it
/// supplies.
fn update_body(case: &UpdateCase, arn: &str) -> Value {
    let mut body = if case.by_arn {
        json!({ "GroupARN": arn })
    } else {
        json!({ "GroupName": case.name })
    };
    apply_configuration(&mut body, case.update);
    body
}

/// Write a configuration's present members into a request body.
fn apply_configuration(body: &mut Value, configuration: Configuration) {
    if let Some(filter) = configuration.filter {
        body["FilterExpression"] = json!(filter);
    }
    if let Some((insights, notifications)) = configuration.insights {
        body["InsightsConfiguration"] =
            json!({"InsightsEnabled": insights, "NotificationsEnabled": notifications});
    }
}

/// The `InsightsConfiguration` a response `Group` reports, as the table states
/// it; absent (or null) means the group carries none.
fn insights_of(group: &Value) -> Option<(bool, bool)> {
    group
        .get("InsightsConfiguration")
        .filter(|value| !value.is_null())
        .map(|value| {
            (
                value
                    .get("InsightsEnabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                value
                    .get("NotificationsEnabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            )
        })
}

/// Every field a group must report after an update, including the identity the
/// update must NOT have changed.
fn assert_group(
    label: &str,
    source: &str,
    group: &Value,
    name: &str,
    arn: &str,
    expected: Configuration,
) {
    assert_eq!(
        group.get("GroupName").and_then(Value::as_str),
        Some(name),
        "case {label:?} via {source}: an update must not change GroupName, got {group:?}"
    );
    assert_eq!(
        group.get("GroupARN").and_then(Value::as_str),
        Some(arn),
        "case {label:?} via {source}: an update must keep the ARN generated upon creation {arn:?}, got {group:?}"
    );
    let actual_filter = group.get("FilterExpression").and_then(Value::as_str);
    assert_eq!(
        actual_filter, expected.filter,
        "case {label:?} via {source}: expected FilterExpression {:?}, got {actual_filter:?} in {group:?}",
        expected.filter
    );
    let actual_insights = insights_of(group);
    assert_eq!(
        actual_insights, expected.insights,
        "case {label:?} via {source}: expected InsightsConfiguration {:?}, got {actual_insights:?} in {group:?}",
        expected.insights
    );
}

#[tokio::test]
async fn an_update_replaces_what_it_supplies_keeps_what_it_omits_and_persists() {
    for case in update_cases() {
        let xray = Xray::new();
        let label = case.case;

        let created = xray.call("/CreateGroup", &create_body(&case)).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: CreateGroup must succeed, got {created:?}"
        );
        let arn = created
            .get("Group")
            .get("GroupARN")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_group_arn_shape(label, case.name, &arn);

        let updated = xray.call("/UpdateGroup", &update_body(&case, &arn)).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateGroup must succeed, got {updated:?}"
        );
        assert_group(
            label,
            "UpdateGroup",
            updated.get("Group"),
            case.name,
            &arn,
            case.expected(),
        );

        // The round trip: the update landed in the store, so both selectors
        // answer the updated record.
        let by_name = xray
            .call("/GetGroup", &json!({"GroupName": case.name}))
            .await;
        assert_eq!(
            by_name.status, 200,
            "case {label:?}: GetGroup by name must succeed, got {by_name:?}"
        );
        assert_group(
            label,
            "GetGroup by GroupName",
            by_name.get("Group"),
            case.name,
            &arn,
            case.expected(),
        );

        let by_arn = xray.call("/GetGroup", &json!({"GroupARN": &arn})).await;
        assert_eq!(
            by_arn.status, 200,
            "case {label:?}: GetGroup by ARN must succeed, got {by_arn:?}"
        );
        assert_eq!(
            by_name.get("Group"),
            by_arn.get("Group"),
            "case {label:?}: both selectors must resolve the same updated record, got {:?} and {:?}",
            by_name.get("Group"),
            by_arn.get("Group")
        );

        // The listing is recomputed from the same record, so the summary
        // carries the updated filter expression too.
        let listed = xray.call("/Groups", &json!({})).await;
        assert_eq!(
            listed.status, 200,
            "case {label:?}: GetGroups must succeed, got {listed:?}"
        );
        let summaries = listed
            .get("Groups")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let summary = summaries
            .iter()
            .find(|group| group.get("GroupName").and_then(Value::as_str) == Some(case.name))
            .cloned()
            .unwrap_or(Value::Null);
        assert_group(
            label,
            "GetGroups",
            &summary,
            case.name,
            &arn,
            case.expected(),
        );
        assert_eq!(
            summaries.len(),
            2,
            "case {label:?}: an update must not create a second group — only the created one and the reserved Default group are listed, got {summaries:?}"
        );

        // Applying the same update again is idempotent.
        let again = xray.call("/UpdateGroup", &update_body(&case, &arn)).await;
        assert_eq!(
            again.status, 200,
            "case {label:?}: repeating an update must succeed, got {again:?}"
        );
        assert_eq!(
            again.get("Group"),
            updated.get("Group"),
            "case {label:?}: repeating an update must answer the same group, got {:?} then {:?}",
            updated.get("Group"),
            again.get("Group")
        );
    }
}

#[tokio::test]
async fn updates_accumulate_in_the_order_they_were_applied() {
    let xray = Xray::new();
    let created = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "sequence", "FilterExpression": "ok"}),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "seeding must succeed, got {created:?}"
    );
    let arn = created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Each step supplies ONE member; the configuration the group ends each step
    // with is the running fold of what was supplied so far.
    let steps: Vec<(&str, Value, Configuration)> = vec![
        (
            "filter only",
            json!({"GroupName": "sequence", "FilterExpression": "fault = true"}),
            Configuration {
                filter: Some("fault = true"),
                insights: None,
            },
        ),
        (
            "insights only, by ARN",
            json!({"GroupARN": &arn, "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": false}}),
            Configuration {
                filter: Some("fault = true"),
                insights: Some((true, false)),
            },
        ),
        (
            "filter replaced",
            json!({"GroupName": "sequence", "FilterExpression": "responsetime > 3"}),
            Configuration {
                filter: Some("responsetime > 3"),
                insights: Some((true, false)),
            },
        ),
        (
            "notifications enabled",
            json!({"GroupARN": &arn, "InsightsConfiguration": {"InsightsEnabled": true, "NotificationsEnabled": true}}),
            Configuration {
                filter: Some("responsetime > 3"),
                insights: Some((true, true)),
            },
        ),
    ];

    for (label, body, expected) in steps {
        let updated = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            updated.status, 200,
            "case {label:?}: UpdateGroup must succeed, got {updated:?}"
        );
        assert_group(label, "UpdateGroup", updated.get("Group"), "sequence", &arn, expected);
        let read = xray
            .call("/GetGroup", &json!({"GroupName": "sequence"}))
            .await;
        assert_group(label, "GetGroup", read.get("Group"), "sequence", &arn, expected);
    }
}

#[tokio::test]
async fn an_update_that_names_no_stored_group_is_refused_and_changes_nothing() {
    let xray = Xray::new();
    let created = xray
        .call(
            "/CreateGroup",
            &json!({"GroupName": "kept", "FilterExpression": "fault = true"}),
        )
        .await;
    assert_eq!(created.status, 200, "seeding must succeed, got {created:?}");
    let arn = created
        .get("Group")
        .get("GroupARN")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Rows span the ways an UpdateGroup request can fail to name a group this
    // emulator holds, plus both published length bounds of each selector
    // (`GroupName` 1..32, `GroupARN` 1..400). A selector outside its MODELLED
    // bound is refused one layer earlier, by the request layer, which answers
    // the `ValidationException` envelope rather than this operation's own
    // modelled error.
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "neither selector",
            json!({"FilterExpression": "fault = true"}),
            "InvalidRequestException",
        ),
        (
            "a GroupName naming no stored group",
            json!({"GroupName": "absent", "FilterExpression": "fault = true"}),
            "InvalidRequestException",
        ),
        (
            "a GroupARN naming no stored group",
            json!({
                "GroupARN": "arn:aws:xray:us-east-1:000000000000:group/absent/0123456789ab",
                "FilterExpression": "fault = true"
            }),
            "InvalidRequestException",
        ),
        (
            "a GroupName below the modelled minimum length",
            json!({"GroupName": "", "FilterExpression": "fault = true"}),
            "ValidationException",
        ),
        (
            "a GroupName above the modelled maximum length",
            json!({"GroupName": "a".repeat(33), "FilterExpression": "fault = true"}),
            "ValidationException",
        ),
        (
            "a GroupARN below the modelled minimum length",
            json!({"GroupARN": "", "FilterExpression": "fault = true"}),
            "ValidationException",
        ),
        (
            "a GroupARN above the modelled maximum length",
            json!({"GroupARN": "a".repeat(401), "FilterExpression": "fault = true"}),
            "ValidationException",
        ),
    ];

    for (label, body, code) in rows {
        let refused = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some(code)),
            "case {label:?}: an update naming no stored group must be refused with 400 {code}, got {refused:?}"
        );
        // A framework refusal renders `message`; a modelled error shape renders
        // its member's own casing (`Message`).
        let message = refused
            .get("message")
            .as_str()
            .or_else(|| refused.get("Message").as_str());
        assert!(
            message.is_some_and(|message| !message.is_empty()),
            "case {label:?}: a refusal must say what was wrong, got {refused:?}"
        );
    }

    // Every refusal left the one stored group exactly as it was, and created
    // nothing: it and the reserved `Default` group are all that is listed.
    let listed = xray.call("/Groups", &json!({})).await;
    let summaries = listed.get("Groups").as_array().cloned().unwrap_or_default();
    assert_eq!(
        summaries.len(),
        2,
        "a refused update must store nothing, got {summaries:?}"
    );
    let kept = summaries
        .iter()
        .find(|group| group.get("GroupName").and_then(Value::as_str) == Some("kept"))
        .cloned()
        .unwrap_or(Value::Null);
    assert_group(
        "after the refusals",
        "GetGroups",
        &kept,
        "kept",
        &arn,
        Configuration {
            filter: Some("fault = true"),
            insights: None,
        },
    );

    // A deleted group's selectors stop resolving, so an update through either
    // is refused rather than re-creating it.
    let deleted = xray
        .call("/DeleteGroup", &json!({"GroupName": "kept"}))
        .await;
    assert_eq!(deleted.status, 200, "DeleteGroup must succeed, got {deleted:?}");
    for (label, body) in [
        (
            "the deleted group's name",
            json!({"GroupName": "kept", "FilterExpression": "error = true"}),
        ),
        (
            "the deleted group's ARN",
            json!({"GroupARN": &arn, "FilterExpression": "error = true"}),
        ),
    ] {
        let refused = xray.call("/UpdateGroup", &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("InvalidRequestException")),
            "case {label:?}: updating a deleted group must be refused, got {refused:?}"
        );
    }
    let empty = xray.call("/Groups", &json!({})).await;
    let remaining: Vec<&str> = empty
        .get("Groups")
        .as_array()
        .map(|groups| {
            groups
                .iter()
                .filter_map(|group| group.get("GroupName").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        remaining,
        vec!["Default"],
        "a refused update must not re-create the deleted group, got {remaining:?}"
    );
}
