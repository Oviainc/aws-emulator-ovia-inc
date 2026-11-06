// SPDX-License-Identifier: CC0-1.0

//! Setting the parameters of a cluster parameter group, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: that EVERY value a request sets becomes the group's own value for
//! that parameter and flips its `Source` to `user` while every other parameter
//! keeps its family's engine default, that the published `AllowedValues` set
//! decides which values are taken (on each published bound and one past it),
//! that the documented per-request parameter count and WLM value length bounds
//! are enforced, that a refused request changes NOTHING, and that a static
//! change leaves every associated cluster `pending-reboot`.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::cluster_parameters::{create_group, default_group_name, reported_parameters};

/// Drive one `ModifyClusterParameterGroup` request setting these name-value
/// pairs, in the order given.
///
/// The awsQuery wire numbers a list member from 1, so position `n` of
/// `parameters` is sent as `Parameters.Parameter.{n + 1}`.
///
/// `pub(crate)` so the sibling reset suite sets the values it then resets
/// through the real write path instead of seeding them.
pub(crate) async fn modify(emulator: &Emulator, group: &str, parameters: &[(&str, &str)]) -> Answer {
    let mut params: Vec<(String, String)> =
        vec![("ParameterGroupName".to_string(), group.to_string())];
    for (position, (name, value)) in parameters.iter().enumerate() {
        let index = position + 1;
        params.push((
            format!("Parameters.Parameter.{index}.ParameterName"),
            (*name).to_string(),
        ));
        params.push((
            format!("Parameters.Parameter.{index}.ParameterValue"),
            (*value).to_string(),
        ));
    }
    let borrowed: Vec<(&str, &str)> = params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .call("ModifyClusterParameterGroup", &borrowed)
        .await
}

/// Every parameter one group reports, as `(name, value, source)` in page order.
///
/// Read back through `DescribeClusterParameters`, so the assertions are about
/// what a LATER caller sees rather than about the store.
///
/// `pub(crate)` so the sibling reset suite reads a group back the same way.
pub(crate) async fn reported(emulator: &Emulator, group: &str) -> Vec<(String, String, String)> {
    let answer = emulator
        .call(
            "DescribeClusterParameters",
            &[("ParameterGroupName", group)],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case read back {group}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    reported_parameters(&answer.body)
        .into_iter()
        .map(|parameter| {
            (
                parameter
                    .name
                    .expect("every reported parameter carries a ParameterName"),
                parameter
                    .value
                    .expect("every reported parameter carries a ParameterValue"),
                parameter
                    .source
                    .expect("every reported parameter carries a Source"),
            )
        })
        .collect()
}

/// The `(value, source)` one group reports for the parameter `name`.
async fn reported_value(emulator: &Emulator, group: &str, name: &str) -> (String, String) {
    reported(emulator, group)
        .await
        .into_iter()
        .find(|(reported, _, _)| reported == name)
        .map(|(_, value, source)| (value, source))
        .unwrap_or_else(|| panic!("case read back {group}: no parameter named {name} is reported"))
}

/// The catalog entry for one parameter of the published family, read off the
/// same catalog the handlers do rather than spelled out here.
///
/// `pub(crate)` so the sibling reset suite reads the same catalog.
pub(crate) fn catalog_entry(name: &str) -> &'static state::EngineParameter {
    state::ENGINE_DEFAULT_PARAMETERS
        .iter()
        .find(|parameter| parameter.name == name)
        .unwrap_or_else(|| panic!("the published catalog holds a parameter named {name}"))
}

/// One row of the set-and-read-back table: the group to set them on, the
/// name-value pairs one request carries, and the values the group must report
/// afterwards.
struct SetCase {
    case: &'static str,
    group: &'static str,
    sent: Vec<(&'static str, &'static str)>,
    expected: Vec<(&'static str, &'static str)>,
}

/// Every value a request sets becomes the group's own value for that parameter,
/// reported with `Source` `user`, while every parameter the request did not name
/// keeps its family's engine default.
///
/// The table spans what the rule generalises over: the parameter's data type
/// (boolean, integer, free text), its apply type (static and dynamic), how many
/// parameters one request carries, a value that happens to equal the engine
/// default, and the same parameter named twice in one request.
#[tokio::test]
async fn every_value_a_request_sets_is_what_the_group_reports_back() {
    let emulator = Emulator::new();
    let cases = vec![
        SetCase {
            case: "one static boolean parameter",
            group: "set-one-boolean",
            sent: vec![("require_ssl", "true")],
            expected: vec![("require_ssl", "true")],
        },
        SetCase {
            case: "one dynamic free-text parameter",
            group: "set-one-free-text",
            sent: vec![("search_path", "$user, public, analytics")],
            expected: vec![("search_path", "$user, public, analytics")],
        },
        SetCase {
            case: "one dynamic integer parameter",
            group: "set-one-integer",
            sent: vec![("max_concurrency_scaling_clusters", "5")],
            expected: vec![("max_concurrency_scaling_clusters", "5")],
        },
        SetCase {
            case: "one static integer parameter",
            group: "set-one-static-integer",
            sent: vec![("statement_timeout", "60000")],
            expected: vec![("statement_timeout", "60000")],
        },
        SetCase {
            case: "the workload management configuration, a static free-text parameter",
            group: "set-wlm",
            sent: vec![(
                "wlm_json_configuration",
                "[{\"user_group\":[\"analyst\"],\"query_concurrency\":5},{\"auto_wlm\":true}]",
            )],
            expected: vec![(
                "wlm_json_configuration",
                "[{\"user_group\":[\"analyst\"],\"query_concurrency\":5},{\"auto_wlm\":true}]",
            )],
        },
        SetCase {
            case: "three parameters in one request",
            group: "set-three",
            sent: vec![
                ("auto_analyze", "false"),
                ("datestyle", "ISO, DMY"),
                ("statement_timeout", "60000"),
            ],
            expected: vec![
                ("auto_analyze", "false"),
                ("datestyle", "ISO, DMY"),
                ("statement_timeout", "60000"),
            ],
        },
        SetCase {
            // A value a caller SET is the caller's, whatever it is: only
            // `ResetClusterParameterGroup` puts a parameter back to
            // `engine-default`.
            case: "a value equal to the family's engine default",
            group: "set-the-default-value",
            sent: vec![("auto_mv", "true")],
            expected: vec![("auto_mv", "true")],
        },
        SetCase {
            case: "the same parameter twice in one request: the later value wins",
            group: "set-twice",
            sent: vec![("query_group", "first"), ("query_group", "second")],
            expected: vec![("query_group", "second")],
        },
    ];

    for case in &cases {
        create_group(&emulator, case.group).await;

        let answer = modify(&emulator, case.group, &case.sent).await;
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

        // ROUND TRIP: the read reports what the write persisted, and nothing
        // else about the group moved.
        let reported = reported(&emulator, case.group).await;
        assert_eq!(
            reported
                .iter()
                .map(|(name, _, _)| name.as_str())
                .collect::<Vec<_>>(),
            state::ENGINE_DEFAULT_PARAMETERS
                .iter()
                .map(|parameter| parameter.name)
                .collect::<Vec<_>>(),
            "case {}: expected the whole catalog in catalog order after the modify, got {:?}",
            case.case,
            reported
        );
        for (name, value, source) in &reported {
            let expected_value = case
                .expected
                .iter()
                .find(|(expected_name, _)| expected_name == name)
                .map(|(_, expected_value)| *expected_value);
            match expected_value {
                Some(expected_value) => assert_eq!(
                    (value.as_str(), source.as_str()),
                    (expected_value, state::PARAMETER_SOURCE_USER),
                    "case {}: expected {name} to report the value the request set with Source user, got {value:?} / {source:?}",
                    case.case
                ),
                None => assert_eq!(
                    (value.as_str(), source.as_str()),
                    (
                        catalog_entry(name).default_value,
                        state::PARAMETER_SOURCE_ENGINE_DEFAULT
                    ),
                    "case {}: expected the untouched {name} to keep its engine default, got {value:?} / {source:?}",
                    case.case
                ),
            }
        }

        // The documented `Source` filter now selects exactly the parameters the
        // request set — the override, not the catalog, is what makes a parameter
        // a `user` one.
        let user_only = emulator
            .call(
                "DescribeClusterParameters",
                &[
                    ("ParameterGroupName", case.group),
                    ("Source", state::PARAMETER_SOURCE_USER),
                ],
            )
            .await;
        let mut expected_names: Vec<&str> = state::ENGINE_DEFAULT_PARAMETERS
            .iter()
            .map(|parameter| parameter.name)
            .filter(|name| {
                case.expected
                    .iter()
                    .any(|(expected_name, _)| expected_name == name)
            })
            .collect();
        expected_names.sort_unstable();
        let mut listed = all_text(&user_only.body, "ParameterName");
        listed.sort_unstable();
        assert_eq!(
            listed,
            expected_names
                .iter()
                .map(|name| (*name).to_string())
                .collect::<Vec<_>>(),
            "case {}: expected the Source=user page to list exactly the parameters the request set, got {listed:?}",
            case.case
        );
    }
}

/// One row of the allowed-values table: the parameter, the value a request
/// carries, and whether the published set admits it.
struct ValueCase {
    case: &'static str,
    parameter: &'static str,
    value: &'static str,
    accepted: bool,
}

/// A value the parameter's published `AllowedValues` set admits is taken, and
/// one it does not is refused with 400 `InvalidParameterValue` while the group
/// keeps the value it had.
///
/// Source: the `AllowedValues` of `state::ENGINE_DEFAULT_PARAMETERS`, which is
/// what `smithy_query(query="doc", shape="Parameter", member="AllowedValues")`
/// ("The valid range of values for the parameter") publishes per parameter.
/// Redshift writes a set as a comma-separated list of literals (`true,false`)
/// or inclusive numeric ranges (`-15-2`, `0-10`, `0,100-2147483647`), so each
/// published range is covered on BOTH bounds and one past each, and a parameter
/// that publishes no set is covered too.
#[tokio::test]
async fn the_published_allowed_values_set_decides_which_values_are_taken() {
    let emulator = Emulator::new();
    const GROUP: &str = "allowed-values";
    create_group(&emulator, GROUP).await;

    let cases = vec![
        // `extra_float_digits`: "-15-2" — one range with a negative low bound.
        ValueCase {
            case: "at the lower bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "-15",
            accepted: true,
        },
        ValueCase {
            case: "at the upper bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "2",
            accepted: true,
        },
        ValueCase {
            case: "inside a range with a negative low",
            parameter: "extra_float_digits",
            value: "0",
            accepted: true,
        },
        ValueCase {
            case: "one below the lower bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "-16",
            accepted: false,
        },
        ValueCase {
            case: "one above the upper bound of a range with a negative low",
            parameter: "extra_float_digits",
            value: "3",
            accepted: false,
        },
        ValueCase {
            case: "not a number at all, for a numeric range",
            parameter: "extra_float_digits",
            value: "two",
            accepted: false,
        },
        // `max_concurrency_scaling_clusters`: "0-10".
        ValueCase {
            case: "at the lower bound",
            parameter: "max_concurrency_scaling_clusters",
            value: "0",
            accepted: true,
        },
        ValueCase {
            case: "at the upper bound",
            parameter: "max_concurrency_scaling_clusters",
            value: "10",
            accepted: true,
        },
        ValueCase {
            case: "one above the upper bound",
            parameter: "max_concurrency_scaling_clusters",
            value: "11",
            accepted: false,
        },
        ValueCase {
            case: "one below the lower bound",
            parameter: "max_concurrency_scaling_clusters",
            value: "-1",
            accepted: false,
        },
        // `statement_timeout`: "0,100-2147483647" — a literal beside a range.
        ValueCase {
            case: "the literal beside a range",
            parameter: "statement_timeout",
            value: "0",
            accepted: true,
        },
        ValueCase {
            case: "at the lower bound of the range beside a literal",
            parameter: "statement_timeout",
            value: "100",
            accepted: true,
        },
        ValueCase {
            case: "at the upper bound of the range beside a literal",
            parameter: "statement_timeout",
            value: "2147483647",
            accepted: true,
        },
        ValueCase {
            case: "one below the lower bound of the range beside a literal",
            parameter: "statement_timeout",
            value: "99",
            accepted: false,
        },
        ValueCase {
            case: "one above the upper bound of the range beside a literal",
            parameter: "statement_timeout",
            value: "2147483648",
            accepted: false,
        },
        ValueCase {
            case: "between the literal and the range",
            parameter: "statement_timeout",
            value: "50",
            accepted: false,
        },
        // `use_fips_ssl`: "true,false" — a set of literals.
        ValueCase {
            case: "one literal of a two-literal set",
            parameter: "use_fips_ssl",
            value: "true",
            accepted: true,
        },
        ValueCase {
            case: "the other literal of a two-literal set",
            parameter: "use_fips_ssl",
            value: "false",
            accepted: true,
        },
        ValueCase {
            case: "a literal in the wrong case",
            parameter: "use_fips_ssl",
            value: "TRUE",
            accepted: false,
        },
        ValueCase {
            case: "a value outside a two-literal set",
            parameter: "use_fips_ssl",
            value: "yes",
            accepted: false,
        },
        // A parameter publishing no set admits whatever the engine parses.
        ValueCase {
            case: "free text, for a parameter publishing no set",
            parameter: "max_cursor_result_set_size",
            value: "16000",
            accepted: true,
        },
        ValueCase {
            case: "free text with punctuation, for a parameter publishing no set",
            parameter: "datestyle",
            value: "SQL, DMY",
            accepted: true,
        },
    ];

    for case in &cases {
        let (before, source_before) = reported_value(&emulator, GROUP, case.parameter).await;
        let answer = modify(&emulator, GROUP, &[(case.parameter, case.value)]).await;
        let (after, source_after) = reported_value(&emulator, GROUP, case.parameter).await;

        if case.accepted {
            assert_eq!(
                answer.status, 200,
                "case {} ({} = {:?}): expected 200, got {} with body {}",
                case.case, case.parameter, case.value, answer.status, answer.body
            );
            assert_eq!(
                (after.as_str(), source_after.as_str()),
                (case.value, state::PARAMETER_SOURCE_USER),
                "case {} ({}): expected the value to be taken with Source user, got {after:?} / {source_after:?}",
                case.case,
                case.parameter
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {} ({} = {:?}): expected 400 InvalidParameterValue, got {} / {:?}",
                case.case,
                case.parameter,
                case.value,
                answer.status,
                answer.error_code()
            );
            assert_eq!(
                (after.as_str(), source_after.as_str()),
                (before.as_str(), source_before.as_str()),
                "case {} ({}): expected a refused request to change nothing, got {after:?} / {source_after:?}",
                case.case,
                case.parameter
            );
        }
    }
}

/// A WLM configuration value of exactly `len` characters.
fn wlm_value_of_length(len: usize) -> String {
    const HEAD: &str = "[{\"auto_wlm\":true,\"comment\":\"";
    const TAIL: &str = "\"}]";
    let padding = len - HEAD.len() - TAIL.len();
    format!("{HEAD}{}{TAIL}", "x".repeat(padding))
}

/// The documented maximum size of the WLM configuration value is enforced, on
/// the bound and one past it.
///
/// Source: `smithy_query(query="doc", shape="Parameter",
/// member="ParameterValue")` — "If ParameterName is wlm_json_configuration,
/// then the maximum size of ParameterValue is 8000 characters."
#[tokio::test]
async fn the_documented_wlm_configuration_value_length_bound_is_enforced() {
    let emulator = Emulator::new();
    const GROUP: &str = "wlm-length";
    create_group(&emulator, GROUP).await;

    let at_bound = wlm_value_of_length(8_000);
    let accepted = modify(
        &emulator,
        GROUP,
        &[(state::WLM_JSON_CONFIGURATION, at_bound.as_str())],
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case at the upper bound (8000 characters): expected 200, got {} with body {}",
        accepted.status, accepted.body
    );
    let (value, source) = reported_value(&emulator, GROUP, state::WLM_JSON_CONFIGURATION).await;
    assert_eq!(
        (value.as_str(), source.as_str()),
        (at_bound.as_str(), state::PARAMETER_SOURCE_USER),
        "case at the upper bound (8000 characters): expected the value to be taken, got {} characters / {source:?}",
        value.chars().count()
    );

    let past_bound = wlm_value_of_length(8_001);
    let refused = modify(
        &emulator,
        GROUP,
        &[(state::WLM_JSON_CONFIGURATION, past_bound.as_str())],
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case one past the upper bound (8001 characters): expected 400 InvalidParameterValue, got {} / {:?}",
        refused.status,
        refused.error_code()
    );
    let (unchanged, _) = reported_value(&emulator, GROUP, state::WLM_JSON_CONFIGURATION).await;
    assert_eq!(
        unchanged, at_bound,
        "case one past the upper bound (8001 characters): expected the refused request to change nothing, got {} characters",
        unchanged.chars().count()
    );
}

/// The documented per-request parameter count is enforced, on the bound and one
/// past it.
///
/// Source: `smithy_query(query="doc", shape="ModifyClusterParameterGroupMessage",
/// member="Parameters")` — "A maximum of 20 parameters can be modified in a
/// single request." The published family publishes fewer parameters than the
/// bound, so the rows reach it by naming one parameter repeatedly, which is also
/// what makes the last value of a repeated name the one that lands.
#[tokio::test]
async fn the_documented_per_request_parameter_count_bound_is_enforced() {
    let emulator = Emulator::new();
    const GROUP: &str = "parameter-count";
    create_group(&emulator, GROUP).await;

    let at_bound: Vec<(&str, String)> = (0..state::MAX_MODIFIABLE_PARAMETERS)
        .map(|index| ("statement_timeout", (100 + index).to_string()))
        .collect();
    let sent: Vec<(&str, &str)> = at_bound
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let accepted = modify(&emulator, GROUP, &sent).await;
    assert_eq!(
        accepted.status, 200,
        "case at the upper bound ({} parameters): expected 200, got {} with body {}",
        state::MAX_MODIFIABLE_PARAMETERS, accepted.status, accepted.body
    );
    let last = (100 + state::MAX_MODIFIABLE_PARAMETERS - 1).to_string();
    let (value, source) = reported_value(&emulator, GROUP, "statement_timeout").await;
    assert_eq!(
        (value.as_str(), source.as_str()),
        (last.as_str(), state::PARAMETER_SOURCE_USER),
        "case at the upper bound ({} parameters): expected the last value sent to land, got {value:?} / {source:?}",
        state::MAX_MODIFIABLE_PARAMETERS
    );

    let past_bound: Vec<(&str, String)> = (0..state::MAX_MODIFIABLE_PARAMETERS + 1)
        .map(|index| ("statement_timeout", (200 + index).to_string()))
        .collect();
    let sent: Vec<(&str, &str)> = past_bound
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect();
    let refused = modify(&emulator, GROUP, &sent).await;
    assert_eq!(
        (refused.status, refused.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case one past the upper bound ({} parameters): expected 400 InvalidParameterValue, got {} / {:?}",
        state::MAX_MODIFIABLE_PARAMETERS + 1,
        refused.status,
        refused.error_code()
    );
    let (unchanged, _) = reported_value(&emulator, GROUP, "statement_timeout").await;
    assert_eq!(
        unchanged, last,
        "case one past the upper bound ({} parameters): expected the refused request to change nothing, got {unchanged:?}",
        state::MAX_MODIFIABLE_PARAMETERS + 1
    );
}

/// A request that names no parameter the group can take is refused and changes
/// nothing — including the two members the documentation requires of every
/// entry, and a parameter the family does not publish.
#[tokio::test]
async fn a_request_naming_no_parameter_the_group_publishes_is_refused() {
    let emulator = Emulator::new();
    const GROUP: &str = "refused-entries";
    create_group(&emulator, GROUP).await;

    // Each row is sent as one form body, because some of them must OMIT a
    // member rather than send it empty.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "a parameter the family does not publish",
            vec![
                ("Parameters.Parameter.1.ParameterName", "no_such_parameter"),
                ("Parameters.Parameter.1.ParameterValue", "1"),
            ],
        ),
        (
            "an empty parameter name",
            vec![
                ("Parameters.Parameter.1.ParameterName", ""),
                ("Parameters.Parameter.1.ParameterValue", "true"),
            ],
        ),
        (
            "no parameter name at all",
            vec![("Parameters.Parameter.1.ParameterValue", "true")],
        ),
        (
            "no parameter value at all",
            vec![("Parameters.Parameter.1.ParameterName", "require_ssl")],
        ),
        (
            "one good entry beside one the family does not publish",
            vec![
                ("Parameters.Parameter.1.ParameterName", "require_ssl"),
                ("Parameters.Parameter.1.ParameterValue", "true"),
                ("Parameters.Parameter.2.ParameterName", "no_such_parameter"),
                ("Parameters.Parameter.2.ParameterValue", "1"),
            ],
        ),
    ];

    for (case, entries) in &cases {
        let mut params: Vec<(&str, &str)> = vec![("ParameterGroupName", GROUP)];
        params.extend(entries.iter().copied());
        let answer = emulator
            .call("ModifyClusterParameterGroup", &params)
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case {case}: expected 400 InvalidParameterValue, got {} / {:?}",
            answer.status,
            answer.error_code()
        );

        // Nothing was written — not even the entry that was good on its own.
        let sources: Vec<String> = reported(&emulator, GROUP)
            .await
            .into_iter()
            .map(|(_, _, source)| source)
            .collect();
        assert!(
            sources
                .iter()
                .all(|source| source == state::PARAMETER_SOURCE_ENGINE_DEFAULT),
            "case {case}: expected a refused request to leave every parameter at its engine default, got {sources:?}"
        );
    }
}

/// A group nobody created cannot be modified: the modelled 404
/// `ClusterParameterGroupNotFound`, whatever the request's parameters are.
#[tokio::test]
async fn modifying_a_group_that_does_not_exist_is_not_found() {
    let emulator = Emulator::new();
    let cases = [
        ("a name nobody created", "no-such-group"),
        (
            "a name the documented syntax admits but nobody created",
            "customised",
        ),
        (
            "the default group's name in the wrong case: a group name is matched as stored",
            "DEFAULT.REDSHIFT-1.0",
        ),
    ];

    for (case, group) in cases {
        let answer = modify(&emulator, group, &[("require_ssl", "true")]).await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, Some("ClusterParameterGroupNotFound".to_string())),
            "case {case} ({group}): expected 404 ClusterParameterGroupNotFound, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

/// A DEFAULT parameter group is Redshift's own and takes no value from a
/// caller: the request is refused and the group still reports its family's
/// engine defaults.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Amazon Redshift
/// parameter groups" — you can't modify parameter values in a default parameter
/// group; to use your own values you create a group of the same family.
#[tokio::test]
async fn the_default_parameter_group_takes_no_value_from_a_caller() {
    let emulator = Emulator::new();
    let group = default_group_name();

    let answer = modify(&emulator, &group, &[("require_ssl", "true")]).await;
    assert_eq!(
        (answer.status, answer.error_code()),
        (400, Some("InvalidParameterValue".to_string())),
        "case the default group ({group}): expected 400 InvalidParameterValue, got {} / {:?}",
        answer.status,
        answer.error_code()
    );

    let sources: Vec<String> = reported(&emulator, &group)
        .await
        .into_iter()
        .map(|(_, _, source)| source)
        .collect();
    assert!(
        sources
            .iter()
            .all(|source| source == state::PARAMETER_SOURCE_ENGINE_DEFAULT),
        "case the default group ({group}): expected every parameter to stay at its engine default, got {sources:?}"
    );
}

/// Create a cluster configured with one parameter group.
///
/// `pub(crate)` so the sibling reset suite drives the same create path.
pub(crate) async fn create_cluster_on(emulator: &Emulator, identifier: &str, group: &str) {
    let answer = emulator
        .call(
            "CreateCluster",
            &[
                ("ClusterIdentifier", identifier),
                ("NodeType", "ra3.xlplus"),
                ("MasterUsername", "admin"),
                ("ClusterParameterGroupName", group),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case create {identifier}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// The `ParameterApplyStatus` values one cluster's parameter group membership
/// reports, and the parameters it names as pending.
///
/// `pub(crate)` so the sibling reset suite reads a membership the same way.
pub(crate) async fn membership_of(
    emulator: &Emulator,
    identifier: &str,
) -> (Vec<String>, Vec<String>) {
    let answer = emulator
        .call("DescribeClusters", &[("ClusterIdentifier", identifier)])
        .await;
    assert_eq!(
        answer.status, 200,
        "case read back {identifier}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    (
        all_text(&answer.body, "ParameterApplyStatus"),
        all_text(&answer.body, "ParameterName"),
    )
}

/// A STATIC parameter change leaves every cluster configured with the group
/// reporting `pending-reboot` for the group and for each changed parameter,
/// while a dynamic-only change leaves the membership `in-sync` and a cluster on
/// another group is untouched.
///
/// Source: `smithy_query(query="doc", operation="ResetClusterParameterGroup")`
/// — "For parameter changes to take effect you must reboot any associated
/// clusters" — and `smithy_query(query="doc", shape="ClusterParameterStatus",
/// member="ParameterApplyStatus")` — "`pending-reboot`: The parameter value will
/// be applied after the cluster reboots", "`in-sync`: The parameter value is in
/// sync with the database".
#[tokio::test]
async fn a_static_change_leaves_every_associated_cluster_pending_reboot() {
    let emulator = Emulator::new();
    create_group(&emulator, "static-change").await;
    create_group(&emulator, "dynamic-change").await;
    create_cluster_on(&emulator, "static-one", "static-change").await;
    create_cluster_on(&emulator, "static-two", "static-change").await;
    create_cluster_on(&emulator, "dynamic-one", "dynamic-change").await;

    // Two static parameters, so the membership names both and the per-parameter
    // list is not a single entry by luck.
    let changed = [("require_ssl", "true"), ("enable_user_activity_logging", "true")];
    for (name, _) in &changed {
        assert_eq!(
            catalog_entry(name).apply_type,
            state::APPLY_TYPE_STATIC,
            "case {name}: the catalog must publish this parameter as static for the case to mean anything"
        );
    }
    let answer = modify(&emulator, "static-change", &changed).await;
    assert_eq!(
        answer.status, 200,
        "case a static change: expected 200, got {} with body {}",
        answer.status, answer.body
    );

    for identifier in ["static-one", "static-two"] {
        let (statuses, mut pending) = membership_of(&emulator, identifier).await;
        assert_eq!(
            statuses,
            vec![state::PARAMETER_APPLY_STATUS_PENDING_REBOOT.to_string(); 3],
            "case a static change ({identifier}): expected the membership and both changed parameters to report pending-reboot, got {statuses:?}"
        );
        pending.sort_unstable();
        let mut expected: Vec<String> = changed
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        expected.sort_unstable();
        assert_eq!(
            pending, expected,
            "case a static change ({identifier}): expected exactly the changed parameters to be named as pending, got {pending:?}"
        );
    }

    // A cluster on another group saw no change at all.
    let (statuses, pending) = membership_of(&emulator, "dynamic-one").await;
    assert_eq!(
        statuses,
        vec![state::PARAMETER_APPLY_STATUS_IN_SYNC.to_string()],
        "case a static change (a cluster on another group): expected in-sync, got {statuses:?}"
    );
    assert!(
        pending.is_empty(),
        "case a static change (a cluster on another group): expected no pending parameter, got {pending:?}"
    );

    // A DYNAMIC parameter is applied without a reboot, so it leaves nothing
    // pending on the cluster configured with its group.
    let dynamic = [("query_group", "report"), ("search_path", "$user, public")];
    for (name, _) in &dynamic {
        assert_eq!(
            catalog_entry(name).apply_type,
            state::APPLY_TYPE_DYNAMIC,
            "case {name}: the catalog must publish this parameter as dynamic for the case to mean anything"
        );
    }
    let answer = modify(&emulator, "dynamic-change", &dynamic).await;
    assert_eq!(
        answer.status, 200,
        "case a dynamic change: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let (statuses, pending) = membership_of(&emulator, "dynamic-one").await;
    assert_eq!(
        statuses,
        vec![state::PARAMETER_APPLY_STATUS_IN_SYNC.to_string()],
        "case a dynamic change: expected the membership to stay in-sync, got {statuses:?}"
    );
    assert!(
        pending.is_empty(),
        "case a dynamic change: expected no pending parameter, got {pending:?}"
    );
    // The values themselves still landed on the group.
    for (name, value) in &dynamic {
        let (reported, source) = reported_value(&emulator, "dynamic-change", name).await;
        assert_eq!(
            (reported.as_str(), source.as_str()),
            (*value, state::PARAMETER_SOURCE_USER),
            "case a dynamic change ({name}): expected the value to be taken with Source user, got {reported:?} / {source:?}"
        );
    }
}
