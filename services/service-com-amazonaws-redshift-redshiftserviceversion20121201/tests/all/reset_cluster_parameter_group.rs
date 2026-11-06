// SPDX-License-Identifier: CC0-1.0

//! Putting the parameters of a cluster parameter group back to their family's
//! engine defaults, end to end over the awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: that a reset is OBSERVABLE — every parameter it puts back reports
//! its family's engine default with `Source` `engine-default` on the next read,
//! while every value the request did not reset stays the caller's — over
//! reset-all and named-subset requests, static and dynamic parameters, and
//! counts from none to the documented maximum; that the reset is idempotent;
//! that the documented per-request bound, the "at least one parameter name"
//! rule, an unpublished parameter name, the default group and a group that does
//! not exist are each refused, changing nothing; and that a static reset leaves
//! every associated cluster `pending-reboot` while a dynamic one does not.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::cluster_parameters::create_group;
use crate::modify_cluster_parameter_group::{
    catalog_entry, create_cluster_on, membership_of, modify, reported,
};

/// Drive one `ResetClusterParameterGroup` request.
///
/// `reset_all` is sent only when it is `Some`, so a row can exercise the
/// documented default of the member as well as each value of it. The awsQuery
/// wire numbers a list member from 1, so position `n` of `parameters` is sent as
/// `Parameters.Parameter.{n + 1}.ParameterName` — "an array of names of
/// parameters to be reset", so only the name is sent.
async fn reset(
    emulator: &Emulator,
    group: &str,
    reset_all: Option<bool>,
    parameters: &[&str],
) -> Answer {
    let mut params: Vec<(String, String)> =
        vec![("ParameterGroupName".to_string(), group.to_string())];
    if let Some(reset_all) = reset_all {
        params.push(("ResetAllParameters".to_string(), reset_all.to_string()));
    }
    for (position, name) in parameters.iter().enumerate() {
        params.push((
            format!("Parameters.Parameter.{}.ParameterName", position + 1),
            (*name).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("ResetClusterParameterGroup", &borrowed)
        .await
}

/// The parameters one group reports with `Source` `user`, sorted by name.
///
/// Read through the documented `Source` filter, which is the page a client that
/// verifies a reset asks for: after a reset of everything it must be empty.
async fn user_sourced(emulator: &Emulator, group: &str) -> Vec<String> {
    let answer = emulator
        .call(
            "DescribeClusterParameters",
            &[
                ("ParameterGroupName", group),
                ("Source", state::PARAMETER_SOURCE_USER),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case read back {group} with Source=user: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let mut names = all_text(&answer.body, "ParameterName");
    names.sort_unstable();
    names
}

/// Set these values on a group through the real write path, so the `user`
/// `Source` a reset then clears was put there the way a caller puts it there.
async fn hold(emulator: &Emulator, group: &str, held: &[(&str, &str)]) {
    if held.is_empty() {
        return;
    }
    let answer = modify(emulator, group, held).await;
    assert_eq!(
        answer.status, 200,
        "case hold {held:?} on {group}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// One row of the reset table: what the group holds first, what the request
/// asks to reset, and the values that must still be the caller's afterwards.
struct ResetCase {
    case: &'static str,
    group: &'static str,
    held: Vec<(&'static str, &'static str)>,
    reset_all: Option<bool>,
    named: Vec<&'static str>,
    /// The overrides the reset must leave in place, as `(name, value)`.
    remaining: Vec<(&'static str, &'static str)>,
}

/// Assert that `group` reports exactly `remaining` as the caller's values, and
/// every other parameter of its family at its engine default sourced
/// `engine-default`.
async fn assert_reported(emulator: &Emulator, case: &str, group: &str, remaining: &[(&str, &str)]) {
    for (name, value, source) in reported(emulator, group).await {
        match remaining.iter().find(|(held, _)| *held == name) {
            Some((_, held_value)) => assert_eq!(
                (value.as_str(), source.as_str()),
                (*held_value, state::PARAMETER_SOURCE_USER),
                "case {case}: expected {name} to keep the value the caller set, got {value:?} / {source:?}"
            ),
            None => assert_eq!(
                (value.as_str(), source.as_str()),
                (
                    catalog_entry(&name).default_value,
                    state::PARAMETER_SOURCE_ENGINE_DEFAULT
                ),
                "case {case}: expected {name} back on its engine default, got {value:?} / {source:?}"
            ),
        }
    }

    let mut expected: Vec<String> = remaining.iter().map(|(name, _)| (*name).to_string()).collect();
    expected.sort_unstable();
    let listed = user_sourced(emulator, group).await;
    assert_eq!(
        listed, expected,
        "case {case}: expected the Source=user page to list exactly {expected:?}, got {listed:?}"
    );
}

/// Every parameter a reset puts back reports its family's engine default with
/// `Source` `engine-default` on the next read, while every value the request
/// did not reset stays the caller's.
///
/// Source: `smithy_query(query="doc", operation="ResetClusterParameterGroup")` —
/// "Sets one or more parameters of the specified parameter group to their
/// default values and sets the source values of the parameters to
/// 'engine-default'. To reset the entire parameter group specify the
/// ResetAllParameters parameter."
///
/// The table spans what the rule generalises over: whether the request resets
/// everything or a named subset, how `ResetAllParameters` is carried (`true`,
/// `false` or left to its documented default), the apply type and data type of
/// the parameters named, how many are named (none, one, several, all of them, a
/// name twice), and a name the group holds no value for.
#[tokio::test]
async fn every_reset_puts_the_named_parameters_back_on_their_engine_default() {
    let emulator = Emulator::new();
    let cases = vec![
        ResetCase {
            case: "reset all, asked for explicitly",
            group: "reset-all-explicit",
            held: vec![("require_ssl", "true"), ("statement_timeout", "60000")],
            reset_all: Some(true),
            named: Vec::new(),
            remaining: Vec::new(),
        },
        ResetCase {
            // "Default: true" on `ResetAllParameters`, for the request that
            // names no parameter at all.
            case: "reset all, left to the documented default",
            group: "reset-all-default",
            held: vec![("require_ssl", "true"), ("statement_timeout", "60000")],
            reset_all: None,
            named: Vec::new(),
            remaining: Vec::new(),
        },
        ResetCase {
            case: "reset all of a group holding nothing",
            group: "reset-all-of-nothing",
            held: Vec::new(),
            reset_all: Some(true),
            named: Vec::new(),
            remaining: Vec::new(),
        },
        ResetCase {
            case: "one named static boolean parameter",
            group: "reset-one-static",
            held: vec![("require_ssl", "true"), ("search_path", "$user, analytics")],
            reset_all: None,
            named: vec!["require_ssl"],
            remaining: vec![("search_path", "$user, analytics")],
        },
        ResetCase {
            case: "one named dynamic free-text parameter",
            group: "reset-one-dynamic",
            held: vec![("require_ssl", "true"), ("query_group", "etl")],
            reset_all: None,
            named: vec!["query_group"],
            remaining: vec![("require_ssl", "true")],
        },
        ResetCase {
            case: "three named of four held",
            group: "reset-three-of-four",
            held: vec![
                ("auto_analyze", "false"),
                ("datestyle", "ISO, DMY"),
                ("statement_timeout", "60000"),
                ("max_concurrency_scaling_clusters", "4"),
            ],
            reset_all: None,
            named: vec!["auto_analyze", "datestyle", "statement_timeout"],
            remaining: vec![("max_concurrency_scaling_clusters", "4")],
        },
        ResetCase {
            case: "every held parameter named, with ResetAllParameters false",
            group: "reset-every-held",
            held: vec![("use_fips_ssl", "true"), ("extra_float_digits", "2")],
            reset_all: Some(false),
            named: vec!["use_fips_ssl", "extra_float_digits"],
            remaining: Vec::new(),
        },
        ResetCase {
            // Resetting a parameter already on its engine default puts it back
            // to what it already reports and touches nothing else.
            case: "a named parameter the group holds no value for",
            group: "reset-unheld",
            held: vec![("require_ssl", "true")],
            reset_all: None,
            named: vec!["auto_mv"],
            remaining: vec![("require_ssl", "true")],
        },
        ResetCase {
            case: "the same name twice in one request",
            group: "reset-name-twice",
            held: vec![("statement_timeout", "60000")],
            reset_all: None,
            named: vec!["statement_timeout", "statement_timeout"],
            remaining: Vec::new(),
        },
        ResetCase {
            // Resetting everything already includes whatever the list names.
            case: "reset all beside a named parameter",
            group: "reset-all-and-named",
            held: vec![("require_ssl", "true"), ("query_group", "etl")],
            reset_all: Some(true),
            named: vec!["require_ssl"],
            remaining: Vec::new(),
        },
    ];

    for case in &cases {
        create_group(&emulator, case.group).await;
        hold(&emulator, case.group, &case.held).await;

        let answer = reset(&emulator, case.group, case.reset_all, &case.named).await;
        assert_eq!(
            answer.status, 200,
            "case {}: expected 200, got {} with body {}",
            case.case, answer.status, answer.body
        );
        assert_eq!(
            answer.text("ParameterGroupName").as_deref(),
            Some(case.group),
            "case {}: expected the group name {:?} back, got {:?}",
            case.case,
            case.group,
            answer.text("ParameterGroupName")
        );
        assert_eq!(
            answer.text("ParameterGroupStatus").as_deref(),
            Some(state::PARAMETER_GROUP_STATUS_UPDATED),
            "case {}: expected the documented ParameterGroupStatus, got {:?}",
            case.case,
            answer.text("ParameterGroupStatus")
        );

        // ROUND TRIP: the read reports what the reset persisted.
        assert_reported(&emulator, case.case, case.group, &case.remaining).await;
    }
}

/// A reset is idempotent: repeating it answers the same thing and leaves the
/// group reporting the same parameters.
#[tokio::test]
async fn a_reset_can_be_repeated_with_the_same_result() {
    let emulator = Emulator::new();
    const GROUP: &str = "reset-twice";
    create_group(&emulator, GROUP).await;
    hold(
        &emulator,
        GROUP,
        &[("require_ssl", "true"), ("query_group", "etl")],
    )
    .await;

    let first = reset(&emulator, GROUP, Some(true), &[]).await;
    let second = reset(&emulator, GROUP, Some(true), &[]).await;
    assert_eq!(
        (
            first.status,
            first.text("ParameterGroupStatus"),
            second.status,
            second.text("ParameterGroupStatus"),
        ),
        (
            200,
            Some(state::PARAMETER_GROUP_STATUS_UPDATED.to_string()),
            200,
            Some(state::PARAMETER_GROUP_STATUS_UPDATED.to_string()),
        ),
        "case repeated: expected both resets to answer 200 with the same status, got {} / {:?} \
         then {} / {:?}",
        first.status,
        first.text("ParameterGroupStatus"),
        second.status,
        second.text("ParameterGroupStatus")
    );
    assert_reported(&emulator, "repeated", GROUP, &[]).await;

    // Resetting a named parameter twice is the same answer too.
    hold(&emulator, GROUP, &[("statement_timeout", "60000")]).await;
    for attempt in ["first", "second"] {
        let answer = reset(&emulator, GROUP, None, &["statement_timeout"]).await;
        assert_eq!(
            answer.status, 200,
            "case repeated named ({attempt}): expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_reported(&emulator, "repeated named", GROUP, &[]).await;
    }
}

/// One row of the refusal table.
struct RefusalCase {
    case: &'static str,
    group: &'static str,
    reset_all: Option<bool>,
    named: Vec<&'static str>,
    status: u16,
    code: &'static str,
}

/// A request the service cannot honour is refused with the modelled answer and
/// changes NOTHING the group holds.
///
/// Source: `smithy_query(query="doc", shape="ResetClusterParameterGroupMessage",
/// member="Parameters")` — "If ResetAllParameters option is not used, then at
/// least one parameter name must be supplied" — and the operation's modelled
/// `ClusterParameterGroupNotFoundFault`, "The parameter group name does not
/// refer to an existing parameter group" (HTTP 404). A parameter the family does
/// not publish, and the account's default parameter group (which the Amazon
/// Redshift Cluster Management Guide, "Amazon Redshift parameter groups", says
/// you cannot change parameter values in), are refusals the operation models no
/// error for, so they carry the query-protocol code the service itself answers:
/// `InvalidParameterValue` for a value it cannot honour, and
/// `InvalidParameterCombination` for a request that names neither a parameter
/// nor a reset of all of them.
#[tokio::test]
async fn a_request_the_service_cannot_honour_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    const GROUP: &str = "reset-refusals";
    create_group(&emulator, GROUP).await;
    let held = [("require_ssl", "true"), ("statement_timeout", "60000")];
    hold(&emulator, GROUP, &held).await;

    let default_group = crate::cluster_parameters::default_group_name();
    let cases = vec![
        RefusalCase {
            case: "no parameter name and ResetAllParameters false",
            group: GROUP,
            reset_all: Some(false),
            named: Vec::new(),
            status: 400,
            code: "InvalidParameterCombination",
        },
        RefusalCase {
            case: "a name the family does not publish",
            group: GROUP,
            reset_all: None,
            named: vec!["no_such_parameter"],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a name from another database engine",
            group: GROUP,
            reset_all: None,
            named: vec!["shared_buffers"],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a published name beside an unpublished one",
            group: GROUP,
            reset_all: None,
            named: vec!["require_ssl", "no_such_parameter"],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "an empty parameter name",
            group: GROUP,
            reset_all: None,
            named: vec![""],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a name in the wrong case",
            group: GROUP,
            reset_all: None,
            named: vec!["REQUIRE_SSL"],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "the account's default parameter group",
            group: "",
            reset_all: Some(true),
            named: Vec::new(),
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalCase {
            case: "a group that was never created",
            group: "no-such-group",
            reset_all: Some(true),
            named: Vec::new(),
            status: 404,
            code: "ClusterParameterGroupNotFound",
        },
        RefusalCase {
            case: "a prefix of a group that exists",
            group: "reset-refusal",
            reset_all: None,
            named: vec!["require_ssl"],
            status: 404,
            code: "ClusterParameterGroupNotFound",
        },
    ];

    for case in &cases {
        // The empty group name stands for the account's default group, whose
        // name is read off the catalog rather than spelled out.
        let group = if case.group.is_empty() {
            default_group.as_str()
        } else {
            case.group
        };
        let answer = reset(&emulator, group, case.reset_all, &case.named).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (case.status, Some(case.code.to_string())),
            "case {}: expected {} {}, got {} / {:?} with body {}",
            case.case,
            case.status,
            case.code,
            answer.status,
            answer.error_code(),
            answer.body
        );

        // A refused request left every value the caller had set in place.
        assert_reported(&emulator, case.case, GROUP, &held).await;
    }
}

/// The documented per-request parameter count bound, at the bound and one past
/// it, with the refused request changing nothing.
///
/// Source: `smithy_query(query="doc", shape="ResetClusterParameterGroupMessage",
/// member="Parameters")` — "Constraints: A maximum of 20 parameters can be reset
/// in a single request."
#[tokio::test]
async fn the_documented_per_request_parameter_count_bound_is_enforced() {
    let emulator = Emulator::new();
    const GROUP: &str = "reset-parameter-count";
    create_group(&emulator, GROUP).await;

    // The published catalog holds fewer parameters than the bound, so the rows
    // repeat one name: the bound is on how many the request CARRIES.
    let held = [("statement_timeout", "60000")];
    hold(&emulator, GROUP, &held).await;
    let at_bound = vec!["statement_timeout"; state::MAX_RESETTABLE_PARAMETERS];
    let accepted = reset(&emulator, GROUP, None, &at_bound).await;
    assert_eq!(
        accepted.status,
        200,
        "case at the upper bound ({} parameters): expected 200, got {} with body {}",
        state::MAX_RESETTABLE_PARAMETERS,
        accepted.status,
        accepted.body
    );
    assert_reported(&emulator, "at the upper bound", GROUP, &[]).await;

    hold(&emulator, GROUP, &held).await;
    let past_bound = vec!["statement_timeout"; state::MAX_RESETTABLE_PARAMETERS + 1];
    let refused = reset(&emulator, GROUP, None, &past_bound).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case one past the upper bound ({} parameters): expected 400 InvalidParameterValue, got {} / {:?}",
        state::MAX_RESETTABLE_PARAMETERS + 1,
        refused.status,
        refused.error_code()
    );
    assert_reported(&emulator, "one past the upper bound", GROUP, &held).await;
}

/// Putting a STATIC parameter back leaves every cluster configured with the
/// group reporting `pending-reboot` for the group and for that parameter, while
/// putting a dynamic one back leaves the membership `in-sync`, and a cluster on
/// another group is untouched either way.
///
/// Source: `smithy_query(query="doc", operation="ResetClusterParameterGroup")` —
/// "For parameter changes to take effect you must reboot any associated
/// clusters" — and `smithy_query(query="doc", shape="ClusterParameterStatus",
/// member="ParameterApplyStatus")` — "`pending-reboot`: The parameter value will
/// be applied after the cluster reboots", "`in-sync`: The parameter value is in
/// sync with the database".
#[tokio::test]
async fn a_static_reset_leaves_every_associated_cluster_pending_reboot() {
    let emulator = Emulator::new();
    create_group(&emulator, "reset-static-group").await;
    create_group(&emulator, "reset-dynamic-group").await;

    // The values are set BEFORE the clusters exist, so each cluster starts out
    // reporting its membership `in-sync` and the reset is what moves it.
    hold(&emulator, "reset-static-group", &[("require_ssl", "true")]).await;
    hold(&emulator, "reset-dynamic-group", &[("query_group", "etl")]).await;
    create_cluster_on(&emulator, "reset-static-one", "reset-static-group").await;
    create_cluster_on(&emulator, "reset-static-two", "reset-static-group").await;
    create_cluster_on(&emulator, "reset-dynamic-one", "reset-dynamic-group").await;

    assert_eq!(
        catalog_entry("require_ssl").apply_type,
        state::APPLY_TYPE_STATIC,
        "case require_ssl: the catalog must publish this parameter as static for the case to \
         mean anything, got {:?}",
        catalog_entry("require_ssl").apply_type
    );
    assert_eq!(
        catalog_entry("query_group").apply_type,
        state::APPLY_TYPE_DYNAMIC,
        "case query_group: the catalog must publish this parameter as dynamic for the case to \
         mean anything, got {:?}",
        catalog_entry("query_group").apply_type
    );

    let answer = reset(&emulator, "reset-static-group", None, &["require_ssl"]).await;
    assert_eq!(
        answer.status, 200,
        "case a static reset: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    for identifier in ["reset-static-one", "reset-static-two"] {
        let (statuses, pending) = membership_of(&emulator, identifier).await;
        assert_eq!(
            (statuses.as_slice(), pending.as_slice()),
            (
                [
                    state::PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string(),
                    state::PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string()
                ]
                .as_slice(),
                ["require_ssl".to_string()].as_slice()
            ),
            "case a static reset ({identifier}): expected the membership and require_ssl to \
             report pending-reboot, got {statuses:?} / {pending:?}"
        );
    }

    // A cluster on another group saw no change at all.
    let (statuses, pending) = membership_of(&emulator, "reset-dynamic-one").await;
    assert_eq!(
        (statuses.as_slice(), pending.as_slice()),
        (
            [state::PARAMETER_APPLY_STATUS_IN_SYNC.to_string()].as_slice(),
            [].as_slice()
        ),
        "case a static reset (a cluster on another group): expected in-sync and nothing \
         pending, got {statuses:?} / {pending:?}"
    );

    // Putting a DYNAMIC parameter back is applied without a reboot, so it
    // leaves nothing pending on the cluster configured with its group.
    let answer = reset(&emulator, "reset-dynamic-group", Some(true), &[]).await;
    assert_eq!(
        answer.status, 200,
        "case a dynamic reset: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let (statuses, pending) = membership_of(&emulator, "reset-dynamic-one").await;
    assert_eq!(
        (statuses.as_slice(), pending.as_slice()),
        (
            [state::PARAMETER_APPLY_STATUS_IN_SYNC.to_string()].as_slice(),
            [].as_slice()
        ),
        "case a dynamic reset: expected the membership to stay in-sync with nothing pending, \
         got {statuses:?} / {pending:?}"
    );
    // The value itself went back to the engine default all the same.
    assert_reported(&emulator, "a dynamic reset", "reset-dynamic-group", &[]).await;
}
