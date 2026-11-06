// SPDX-License-Identifier: CC0-1.0

//! The parameters a cluster parameter group reports, end to end over the
//! awsQuery wire.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds (the harness is shared with
//! [`crate::cluster_lifecycle`]), so each assertion covers the whole walk:
//! form-urlencoded deserialize -> handler -> XML serialize.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request: that EVERY group of a published family reports that family's engine
//! default catalog in `ParameterName` order, that a value a caller holds on the
//! group replaces the default and flips the parameter's `Source` to `user`, that
//! the documented `Source` filter keeps exactly the matching parameter types and
//! refuses anything else, the documented `MaxRecords` window on each bound and
//! one past it, the `Marker` cursor, the not-found fault, and that a read leaves
//! the store exactly as it was.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::ops::types::ClusterParameterGroup;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// The parameter group family of the engine version this service publishes,
/// read off the same catalog the handlers do rather than spelled out here.
///
/// `pub(crate)` so a sibling suite under `tests/all/` names the family the same
/// way instead of spelling it out.
pub(crate) fn default_family() -> String {
    state::parameter_group_family(state::DEFAULT_CLUSTER_VERSION)
}

/// The name of that engine version's DEFAULT parameter group, which a fresh
/// account already has.
pub(crate) fn default_group_name() -> String {
    state::default_parameter_group(state::DEFAULT_CLUSTER_VERSION)
}

/// One `<Parameter>` of a `DescribeClusterParameters` body, reduced to the
/// members the assertions read.
#[derive(Debug)]
pub(crate) struct Reported {
    pub(crate) name: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) data_type: Option<String>,
    pub(crate) allowed_values: Option<String>,
    pub(crate) apply_type: Option<String>,
    pub(crate) is_modifiable: Option<String>,
    pub(crate) minimum_engine_version: Option<String>,
}

/// The first `<tag>` inside one parameter's XML, or `None` when the member is
/// absent from the response.
fn member(block: &str, tag: &str) -> Option<String> {
    all_text(block, tag).into_iter().next()
}

/// The inner XML of every `<Parameter>` element of a body, in document order.
///
/// Matched on the element NAME rather than on a fixed `<Parameter>` open tag,
/// because the framework's serializer repeats the service's XML namespace as an
/// attribute on the first member of a list (`<Parameter xmlns="…">`), and
/// because `<ParameterName>` and friends must not be mistaken for list members.
fn parameter_blocks(xml: &str) -> Vec<&str> {
    const OPEN: &str = "<Parameter";
    const CLOSE: &str = "</Parameter>";
    let mut blocks = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(OPEN) {
        let after = &rest[start + OPEN.len()..];
        // The element is `Parameter` itself only when the name ends here: at the
        // tag's own `>` or at the whitespace before an attribute.
        if !(after.starts_with('>') || after.starts_with(' ')) {
            rest = after;
            continue;
        }
        let Some(head_end) = after.find('>') else { break };
        let inner = &after[head_end + 1..];
        let Some(end) = inner.find(CLOSE) else { break };
        blocks.push(&inner[..end]);
        rest = &inner[end + CLOSE.len()..];
    }
    blocks
}

/// Every parameter a `DescribeClusterParameters` body carries, in the order the
/// page lists them.
///
/// Read one `<Parameter>` element at a time rather than one member list at a
/// time, because a member the service publishes no value for (`AllowedValues`
/// of a free-text parameter) is simply absent, so per-member lists would not
/// line up.
///
/// `pub(crate)` so a sibling suite reads one body the same way.
pub(crate) fn reported_parameters(xml: &str) -> Vec<Reported> {
    parameter_blocks(xml)
        .into_iter()
        .map(|block| Reported {
            name: member(block, "ParameterName"),
            value: member(block, "ParameterValue"),
            description: member(block, "Description"),
            source: member(block, "Source"),
            data_type: member(block, "DataType"),
            allowed_values: member(block, "AllowedValues"),
            apply_type: member(block, "ApplyType"),
            is_modifiable: member(block, "IsModifiable"),
            minimum_engine_version: member(block, "MinimumEngineVersion"),
        })
        .collect()
}

/// The `ParameterName` of every parameter a body carries, in page order.
fn reported_names(xml: &str) -> Vec<String> {
    reported_parameters(xml)
        .into_iter()
        .map(|parameter| {
            parameter
                .name
                .expect("every reported parameter carries a ParameterName")
        })
        .collect()
}

/// The published catalog's names, in catalog order.
fn catalog_names() -> Vec<&'static str> {
    state::ENGINE_DEFAULT_PARAMETERS
        .iter()
        .map(|parameter| parameter.name)
        .collect()
}

/// Create one parameter group of the published family.
///
/// `pub(crate)` so a sibling suite that needs a group of the account's own
/// drives the same create path.
pub(crate) async fn create_group(emulator: &Emulator, name: &str) {
    let answer = emulator
        .call(
            "CreateClusterParameterGroup",
            &[
                ("ParameterGroupName", name),
                ("ParameterGroupFamily", default_family().as_str()),
                ("Description", "a group for the parameter suite"),
            ],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case create {name}: expected 200, got {} with body {}",
        answer.status, answer.body
    );
}

/// Persist values a caller holds on one group, straight into the same shared
/// store the handler reads.
///
/// This suite is about what a READ reports, so it seeds the held values rather
/// than driving `ModifyClusterParameterGroup` for them: the `user` `Source` is
/// then observable without the read's assertions depending on the write path,
/// which has its own suite in
/// [`crate::modify_cluster_parameter_group`]. Exactly as
/// `cluster_lifecycle`'s automated-snapshot seeding does for snapshots.
fn seed_held_values(emulator: &Emulator, group_name: &str, held: &[(&str, &str)]) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    let group = store
        .parameter_groups
        .iter_mut()
        .find(|group| group.group.parameter_group_name.as_deref() == Some(group_name))
        .expect("the group to seed values on exists");
    for (name, value) in held {
        group
            .parameter_values
            .insert((*name).to_string(), (*value).to_string());
    }
}

/// Seed a parameter group whose family no published engine version offers.
///
/// `CreateClusterParameterGroup` refuses such a family, so the record can only
/// be put in the store directly.
fn seed_group_of_unknown_family(emulator: &Emulator, group_name: &str, family: &str) {
    let store = state::service_state(&*emulator.engine).expect("the Redshift store");
    let mut store = store.lock();
    store
        .parameter_groups
        .push(state::StoredParameterGroup {
            group: ClusterParameterGroup {
                parameter_group_name: Some(group_name.to_string()),
                parameter_group_family: Some(family.to_string()),
                description: Some("a group of a family no version publishes".to_string()),
                tags: Some(Vec::new()),
            },
            default_group: false,
            parameter_values: std::collections::BTreeMap::new(),
        });
}

/// Every parameter group of a published family reports that family's engine
/// default catalog, whole and in `ParameterName` order, with every member the
/// catalog publishes for each entry.
///
/// The table spans what the rule generalises over: the DEFAULT group a fresh
/// account already has, a group the account created, and a group whose
/// requested name was folded to lower case on the way in.
#[tokio::test]
async fn every_group_of_a_published_family_reports_the_engine_default_catalog() {
    let emulator = Emulator::new();
    create_group(&emulator, "parameters-plain").await;
    create_group(&emulator, "Parameters-Folded").await;

    // Page order is the catalog's order, so the catalog must itself be in
    // `ParameterName` order for a `Marker` to be a stable offset.
    let names = catalog_names();
    assert!(
        !names.is_empty(),
        "case catalog: the published catalog must not be empty, got {names:?}"
    );
    assert!(
        names.windows(2).all(|pair| pair[0] < pair[1]),
        "case catalog: expected the published catalog in ParameterName order, got {names:?}"
    );

    let cases: Vec<(&str, String)> = vec![
        ("the account's default group", default_group_name()),
        ("a created group", "parameters-plain".to_string()),
        // "This value is stored as a lower-case string", so the stored name is
        // the folded one and that is the name a read is keyed on.
        ("a created group, name folded", "parameters-folded".to_string()),
    ];

    for (case, group_name) in cases {
        let answer = emulator
            .call(
                "DescribeClusterParameters",
                &[("ParameterGroupName", group_name.as_str())],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        let reported = reported_parameters(&answer.body);
        assert_eq!(
            reported_names(&answer.body),
            names.iter().map(|name| name.to_string()).collect::<Vec<_>>(),
            "case {case}: expected the whole catalog in order, got {:?}",
            reported_names(&answer.body)
        );

        for (reported, entry) in reported.iter().zip(state::ENGINE_DEFAULT_PARAMETERS) {
            let name = entry.name;
            assert_eq!(
                reported.value.as_deref(),
                Some(entry.default_value),
                "case {case}/{name}: expected the engine default value {:?}, got {:?}",
                entry.default_value,
                reported.value
            );
            assert_eq!(
                reported.description.as_deref(),
                Some(entry.description),
                "case {case}/{name}: expected the published description, got {:?}",
                reported.description
            );
            assert_eq!(
                reported.data_type.as_deref(),
                Some(entry.data_type),
                "case {case}/{name}: expected DataType {:?}, got {:?}",
                entry.data_type,
                reported.data_type
            );
            assert_eq!(
                reported.allowed_values.as_deref(),
                entry.allowed_values,
                "case {case}/{name}: expected AllowedValues {:?}, got {:?}",
                entry.allowed_values,
                reported.allowed_values
            );
            assert_eq!(
                reported.apply_type.as_deref(),
                Some(entry.apply_type),
                "case {case}/{name}: expected ApplyType {:?}, got {:?}",
                entry.apply_type,
                reported.apply_type
            );
            assert_eq!(
                reported.is_modifiable.as_deref(),
                Some(if entry.is_modifiable { "true" } else { "false" }),
                "case {case}/{name}: expected IsModifiable {}, got {:?}",
                entry.is_modifiable,
                reported.is_modifiable
            );
            // Nobody has changed a value on any of these groups, so every
            // parameter is still the one the engine set.
            assert_eq!(
                reported.source.as_deref(),
                Some(state::PARAMETER_SOURCE_ENGINE_DEFAULT),
                "case {case}/{name}: expected Source {:?}, got {:?}",
                state::PARAMETER_SOURCE_ENGINE_DEFAULT,
                reported.source
            );
            assert_eq!(
                reported.minimum_engine_version,
                state::minimum_engine_version(),
                "case {case}/{name}: expected MinimumEngineVersion {:?}, got {:?}",
                state::minimum_engine_version(),
                reported.minimum_engine_version
            );
        }
    }
}

/// The documented default values, data types and apply types of entries the
/// issue cited none of, asserted over the wire.
///
/// Source: the Amazon Redshift Cluster Management Guide, "Amazon Redshift
/// parameter groups" — the default-parameter table and its static/dynamic
/// apply types, which is what `state::ENGINE_DEFAULT_PARAMETERS` is built from.
#[tokio::test]
async fn the_published_catalog_carries_the_documented_settings() {
    let emulator = Emulator::new();
    let answer = emulator
        .call(
            "DescribeClusterParameters",
            &[("ParameterGroupName", default_group_name().as_str())],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case the default group: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let reported = reported_parameters(&answer.body);

    // name, value, data type, allowed values, apply type.
    let expected: [(&str, &str, &str, Option<&str>, &str); 7] = [
        ("search_path", "$user, public", "string", None, "dynamic"),
        ("datestyle", "ISO, MDY", "string", None, "dynamic"),
        (
            "statement_timeout",
            "0",
            "integer",
            Some("0,100-2147483647"),
            "static",
        ),
        (
            "max_concurrency_scaling_clusters",
            "1",
            "integer",
            Some("0-10"),
            "dynamic",
        ),
        (
            "require_ssl",
            "false",
            "boolean",
            Some("true,false"),
            "static",
        ),
        (
            "enable_case_sensitive_identifier",
            "false",
            "boolean",
            Some("true,false"),
            "static",
        ),
        (
            "wlm_json_configuration",
            "[{\"auto_wlm\":true}]",
            "string",
            None,
            "static",
        ),
    ];

    for (name, value, data_type, allowed_values, apply_type) in expected {
        let parameter = reported
            .iter()
            .find(|parameter| parameter.name.as_deref() == Some(name))
            .unwrap_or_else(|| {
                panic!("case {name}: the catalog must publish it, got {reported:?}")
            });
        assert_eq!(
            (
                parameter.value.as_deref(),
                parameter.data_type.as_deref(),
                parameter.allowed_values.as_deref(),
                parameter.apply_type.as_deref(),
            ),
            (Some(value), Some(data_type), allowed_values, Some(apply_type)),
            "case {name}: expected ({value:?}, {data_type:?}, {allowed_values:?}, \
             {apply_type:?}), got {parameter:?}"
        );
    }

    // Both values the `ParameterApplyType` enum declares are published, so a
    // caller can tell a change that needs a reboot from one that does not.
    for apply_type in [state::APPLY_TYPE_STATIC, state::APPLY_TYPE_DYNAMIC] {
        assert!(
            reported
                .iter()
                .any(|parameter| parameter.apply_type.as_deref() == Some(apply_type)),
            "case {apply_type}: expected at least one parameter with this ApplyType, \
             got {:?}",
            reported
                .iter()
                .filter_map(|parameter| parameter.apply_type.clone())
                .collect::<Vec<_>>()
        );
    }
}

/// A value a group holds replaces that parameter's engine default and makes its
/// `Source` `user`; every other parameter is untouched.
///
/// The table spans how many values a group holds and whether they are static or
/// dynamic parameters.
#[tokio::test]
async fn a_value_a_group_holds_replaces_the_engine_default_and_sources_to_user() {
    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("no held value", &[]),
        ("one dynamic parameter", &[("statement_timeout", "60000")]),
        (
            "one static parameter",
            &[("wlm_json_configuration", "[{\"query_group\":[\"etl\"]}]")],
        ),
        (
            "a static and a dynamic parameter",
            &[
                ("enable_user_activity_logging", "true"),
                ("search_path", "$user, public, staging"),
            ],
        ),
    ];

    for (case, held) in cases {
        // One emulator per row: the rows differ in what the group holds, not in
        // what a previous row left behind.
        let emulator = Emulator::new();
        create_group(&emulator, "held-values").await;
        seed_held_values(&emulator, "held-values", held);

        let answer = emulator
            .call(
                "DescribeClusterParameters",
                &[("ParameterGroupName", "held-values")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        for parameter in reported_parameters(&answer.body) {
            let name = parameter.name.clone().unwrap_or_default();
            let entry = state::ENGINE_DEFAULT_PARAMETERS
                .iter()
                .find(|entry| entry.name == name)
                .unwrap_or_else(|| panic!("case {case}: {name} is not a catalog entry"));
            let held_value = held
                .iter()
                .find(|(held_name, _)| *held_name == name)
                .map(|(_, value)| *value);
            let (expected_value, expected_source) = match held_value {
                Some(value) => (value, state::PARAMETER_SOURCE_USER),
                None => (entry.default_value, state::PARAMETER_SOURCE_ENGINE_DEFAULT),
            };
            assert_eq!(
                (parameter.value.as_deref(), parameter.source.as_deref()),
                (Some(expected_value), Some(expected_source)),
                "case {case}/{name}: expected ({expected_value:?}, {expected_source:?}), \
                 got ({:?}, {:?})",
                parameter.value,
                parameter.source
            );
        }

        // The `Source` filter is the same rule seen the other way round: `user`
        // is exactly what the group holds, `engine-default` exactly the rest,
        // and an unfiltered read the two together.
        let held_names: Vec<String> = catalog_names()
            .into_iter()
            .filter(|name| held.iter().any(|(held_name, _)| held_name == name))
            .map(str::to_string)
            .collect();
        let untouched_names: Vec<String> = catalog_names()
            .into_iter()
            .filter(|name| !held.iter().any(|(held_name, _)| held_name == name))
            .map(str::to_string)
            .collect();
        let all_names: Vec<String> = catalog_names().into_iter().map(str::to_string).collect();

        for (filter, expected) in [
            (Some(state::PARAMETER_SOURCE_USER), held_names),
            (
                Some(state::PARAMETER_SOURCE_ENGINE_DEFAULT),
                untouched_names,
            ),
            (None, all_names),
        ] {
            let mut params = vec![("ParameterGroupName", "held-values")];
            if let Some(filter) = filter {
                params.push(("Source", filter));
            }
            let answer = emulator.call("DescribeClusterParameters", &params).await;
            assert_eq!(
                answer.status, 200,
                "case {case}/Source={filter:?}: expected 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                reported_names(&answer.body),
                expected,
                "case {case}/Source={filter:?}: expected {expected:?}, got {:?}",
                reported_names(&answer.body)
            );
        }
    }
}

/// A `Source` outside the documented value set names no parameter type and is
/// refused.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterParametersMessage",
/// member="Source")` — "Valid Values: user | engine-default".
#[tokio::test]
async fn a_source_outside_the_documented_values_is_refused() {
    let emulator = Emulator::new();

    for (case, source) in [
        ("the wrong case", "User"),
        ("the wrong case, filter value", "ENGINE-DEFAULT"),
        ("an underscore for the hyphen", "engine_default"),
        ("a value the member does not declare", "all"),
        ("a value from another member", "default"),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterParameters",
                &[
                    ("ParameterGroupName", default_group_name().as_str()),
                    ("Source", source),
                ],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case {case}: expected Source={source:?} to be refused with 400 \
             InvalidParameterValue, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

/// The documented `MaxRecords` window, on each published bound and one past it,
/// and the `Marker` cursor.
///
/// Source: `smithy_query(query="doc", shape="DescribeClusterParametersMessage",
/// member="MaxRecords")` — "Default: 100 Constraints: minimum 20, maximum 100."
#[tokio::test]
async fn parameters_are_paged_over_the_documented_window() {
    let emulator = Emulator::new();

    for (case, max_records, accepted) in [
        ("zero", "0", false),
        ("negative", "-1", false),
        ("one below the documented minimum", "19", false),
        ("the documented minimum", "20", true),
        ("inside the window", "50", true),
        ("the documented maximum", "100", true),
        ("one above the documented maximum", "101", true),
        ("far above the documented maximum", "1000", true),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterParameters",
                &[
                    ("ParameterGroupName", default_group_name().as_str()),
                    ("MaxRecords", max_records),
                ],
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case}: expected MaxRecords={max_records} to be served with 200, \
                 got {} with body {}",
                answer.status, answer.body
            );
            // Every published family carries fewer parameters than the smallest
            // page the member admits, so an accepted page is the whole catalog
            // and no cursor is handed back.
            assert_eq!(
                reported_names(&answer.body),
                catalog_names()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>(),
                "case {case}: expected the whole catalog on one page, got {:?}",
                reported_names(&answer.body)
            );
            assert_eq!(
                answer.text("Marker"),
                None,
                "case {case}: no records remain, so the page must carry no cursor, got {:?}",
                answer.text("Marker")
            );
        } else {
            assert_eq!(
                (answer.status, answer.error_code()),
                (400, Some("InvalidParameterValue".to_string())),
                "case {case}: expected MaxRecords={max_records} to be refused with 400 \
                 InvalidParameterValue, got {} / {:?}",
                answer.status,
                answer.error_code()
            );
        }
    }

    // A cursor this service did not hand out is refused rather than served as a
    // mid-page offset.
    for (case, cursor) in [
        ("not a cursor at all", "bogus"),
        ("a bare offset", "0"),
        ("an offset with the wrong tag", "0.deadbeef"),
        ("an offset past the catalog", "9999.deadbeef"),
    ] {
        let answer = emulator
            .call(
                "DescribeClusterParameters",
                &[
                    ("ParameterGroupName", default_group_name().as_str()),
                    ("Marker", cursor),
                ],
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (400, Some("InvalidParameterValue".to_string())),
            "case {case}: expected the cursor {cursor:?} to be refused with 400 \
             InvalidParameterValue, got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

/// A name that names no parameter group is answered with the modelled
/// not-found fault, whatever else the request carries.
#[tokio::test]
async fn a_name_that_names_no_group_is_not_found() {
    let emulator = Emulator::new();
    create_group(&emulator, "exists").await;

    for (case, name) in [
        ("never created", "no-such-group"),
        (
            "the default group of a family no version publishes",
            "default.redshift-9.9",
        ),
        ("a prefix of a group that exists", "exist"),
        ("a suffix of a group that exists", "xists"),
    ] {
        let answer = emulator
            .call("DescribeClusterParameters", &[("ParameterGroupName", name)])
            .await;
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, Some("ClusterParameterGroupNotFound".to_string())),
            "case {case}: expected 404 ClusterParameterGroupNotFound for {name:?}, \
             got {} / {:?}",
            answer.status,
            answer.error_code()
        );
    }
}

/// A group whose family no published engine version offers reports no
/// parameters — never another family's.
#[tokio::test]
async fn a_group_of_an_unpublished_family_reports_no_parameters() {
    let emulator = Emulator::new();
    seed_group_of_unknown_family(&emulator, "from-elsewhere", "redshift-0.9");

    let answer = emulator
        .call(
            "DescribeClusterParameters",
            &[("ParameterGroupName", "from-elsewhere")],
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case an unpublished family: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        reported_names(&answer.body),
        Vec::<String>::new(),
        "case an unpublished family: expected no parameters, got {:?}",
        reported_names(&answer.body)
    );
}

/// Reading the parameters changes nothing a later caller sees: the same request
/// answers the same thing, and what the group holds is left exactly as it was.
#[tokio::test]
async fn reading_the_parameters_changes_nothing() {
    let emulator = Emulator::new();
    create_group(&emulator, "read-twice").await;
    seed_held_values(&emulator, "read-twice", &[("require_ssl", "true")]);

    let first = emulator
        .call(
            "DescribeClusterParameters",
            &[("ParameterGroupName", "read-twice")],
        )
        .await;
    // A filtered read in between, so the second unfiltered read also proves the
    // filter left nothing behind.
    let filtered = emulator
        .call(
            "DescribeClusterParameters",
            &[
                ("ParameterGroupName", "read-twice"),
                ("Source", state::PARAMETER_SOURCE_USER),
            ],
        )
        .await;
    let second = emulator
        .call(
            "DescribeClusterParameters",
            &[("ParameterGroupName", "read-twice")],
        )
        .await;

    assert_eq!(
        reported_names(&filtered.body),
        vec!["require_ssl".to_string()],
        "case the filtered read: expected only the held parameter, got {:?}",
        reported_names(&filtered.body)
    );
    // The bodies differ only in the per-request id the envelope carries, so the
    // comparison is over what the two reads REPORT about the group.
    let reported = |xml: &str| {
        reported_parameters(xml)
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value, parameter.source))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        reported(&first.body),
        reported(&second.body),
        "case read twice: expected an identical report, got {:?} then {:?}",
        reported(&first.body),
        reported(&second.body)
    );

    // And the group itself is unchanged for the sibling read.
    let groups = emulator
        .call(
            "DescribeClusterParameterGroups",
            &[("ParameterGroupName", "read-twice")],
        )
        .await;
    assert_eq!(
        (groups.status, groups.text("ParameterGroupFamily")),
        (200, Some(default_family())),
        "case the sibling read: expected the group still of family {:?}, got {} / {:?}",
        default_family(),
        groups.status,
        groups.text("ParameterGroupFamily")
    );
}
