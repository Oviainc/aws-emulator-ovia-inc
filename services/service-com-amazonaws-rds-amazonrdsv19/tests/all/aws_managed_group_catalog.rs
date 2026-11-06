// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the groups Amazon RDS provides ITSELF in every account and region:
//! the default DB parameter groups and the default option groups.
//!
//! Every account has one default DB parameter group per DB parameter group family and one
//! default option group per engine and major engine version, created by no request. The
//! rule under test is that consequence: those groups are reported beside the account's own,
//! they carry the members their wire shapes publish, and a request that would delete or
//! change one is refused rather than answered as a group that does not exist.
//!
//! The rules are asserted over a TABLE spanning what they generalise over rather than over
//! one group: the rows are engines of DIFFERENT products whose major-version forms differ
//! (`8.0`, `17`, `19`, `15.00`, `11.5`, `10.11`), and neither the family nor the major
//! version is written here — each row reads them out of
//! `DescribeDBEngineVersions(Engine=…, DefaultOnly=true)`, so the catalog itself decides
//! which groups must exist.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it, each request is a real awsQuery form POST carrying
//! the SigV4 credential scope the router keys on, and every assertion reads the XML the
//! emulator answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, subtrees, text, texts, REGION};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The engines the table runs over: one per database product this front door serves whose
/// option groups the model's own `EngineName` value list admits, so each row exercises the
/// parameter-group half and the option-group half at once.
const ENGINES: [&str; 6] = [
    "mysql",
    "postgres",
    "oracle-se2",
    "sqlserver-ex",
    "mariadb",
    "db2-ae",
];

/// The `DBParameterGroupFamily` and `MajorEngineVersion` this service publishes for
/// `engine`'s default version, read out of the report that publishes them.
async fn published_default_version(router: &MultiServiceRouter, engine: &str) -> (String, String) {
    let body = expect_ok(
        engine,
        call(
            router,
            "DescribeDBEngineVersions",
            &[("Engine", engine), ("DefaultOnly", "true")],
        )
        .await,
    );
    let entry = subtrees(&body, "DBEngineVersion")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {engine:?}: the engine must publish a default version, got {body}"));
    (
        text(&entry, "DBParameterGroupFamily"),
        text(&entry, "MajorEngineVersion"),
    )
}

/// The name RDS gives the default DB parameter group of `family`: `default.` followed by
/// the family ("Amazon RDS creates a default DB parameter group for each DB engine and
/// version", named `default.<family>`).
fn default_parameter_group_name(family: &str) -> String {
    format!("default.{family}")
}

/// The name RDS gives the default option group of one engine and major engine version:
/// `default:<engine>-<major engine version>` with the version's dots written as hyphens
/// (`default:mysql-8-0`, `default:sqlserver-se-11-00`), as the "Working with option groups"
/// section of the Amazon RDS User Guide publishes it.
fn default_option_group_name(engine: &str, major_engine_version: &str) -> String {
    format!("default:{engine}-{}", major_engine_version.replace('.', "-"))
}

/// `DescribeDBParameterGroups` naming one group.
async fn describe_parameter_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(
        router,
        "DescribeDBParameterGroups",
        &[("DBParameterGroupName", name)],
    )
    .await
}

/// `DescribeOptionGroups` naming one group.
async fn describe_option_group(router: &MultiServiceRouter, name: &str) -> (u16, String) {
    call(router, "DescribeOptionGroups", &[("OptionGroupName", name)]).await
}

/// Each engine's default version names a family and a major version, and the account holds
/// the two groups RDS provides for them: the default DB parameter group of the family and
/// the default option group of the engine and major version. Both describe, both carry the
/// members their shapes publish, and neither can be deleted or changed.
#[tokio::test]
async fn the_account_holds_the_default_group_of_every_published_family() {
    let router = emulator();

    for engine in ENGINES {
        let (family, major_engine_version) = published_default_version(&router, engine).await;
        let case = format!("{engine} {family}/{major_engine_version}");

        // The default DB parameter group of the family.
        let name = default_parameter_group_name(&family);
        let described = expect_ok(&case, describe_parameter_group(&router, &name).await);
        let reported = (
            text(&described, "DBParameterGroupName"),
            text(&described, "DBParameterGroupFamily"),
            text(&described, "Description"),
            text(&described, "DBParameterGroupArn"),
        );
        assert_eq!(
            reported,
            (
                name.clone(),
                family.clone(),
                format!("Default parameter group for {family}"),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:pg:{name}"),
            ),
            "case {case:?}: the default group must report the family it belongs to, got \
             {described}"
        );

        // "You can't delete a default DB parameter group": the group is RDS's, so the
        // refusal reports the restriction rather than a group that does not exist.
        let refused = expect_error(
            &case,
            call(
                &router,
                "DeleteDBParameterGroup",
                &[("DBParameterGroupName", name.as_str())],
            )
            .await,
            400,
            "InvalidDBParameterGroupState",
        );
        let message = text(&refused, "Message");
        assert_eq!(
            message,
            format!("Default DBParameterGroup cannot be deleted: {name}"),
            "case {case:?}: expected the default-group refusal naming it, got {refused}"
        );
        // Nor can it be changed.
        let refused = expect_error(
            &case,
            call(
                &router,
                "ModifyDBParameterGroup",
                &[
                    ("DBParameterGroupName", name.as_str()),
                    ("Parameters.Parameter.1.ParameterName", "autocommit"),
                    ("Parameters.Parameter.1.ParameterValue", "0"),
                ],
            )
            .await,
            400,
            "InvalidParameterValue",
        );
        assert_eq!(
            text(&refused, "Message"),
            "Cannot modify a default parameter group.",
            "case {case:?}: expected the fixed default-group modify refusal, got {refused}"
        );
        // And it is still there after both refusals.
        expect_ok(&case, describe_parameter_group(&router, &name).await);

        // The default option group of the engine and major version.
        let name = default_option_group_name(engine, &major_engine_version);
        let described = expect_ok(&case, describe_option_group(&router, &name).await);
        let reported = (
            text(&described, "OptionGroupName"),
            text(&described, "EngineName"),
            text(&described, "MajorEngineVersion"),
            text(&described, "OptionGroupArn"),
            text(&described, "AllowsVpcAndNonVpcInstanceMemberships"),
        );
        assert_eq!(
            reported,
            (
                name.clone(),
                engine.to_string(),
                major_engine_version.clone(),
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:og:{name}"),
                // A group holding no option constrains nothing, so it can be applied to
                // instances of both network kinds.
                "true".to_string(),
            ),
            "case {case:?}: the default option group must report its engine and version, got \
             {described}"
        );
        assert_eq!(
            text(&described, "OptionGroupDescription"),
            format!("Default option group for {engine} {major_engine_version}"),
            "case {case:?}: got {described}"
        );
        let refused = expect_error(
            &case,
            call(
                &router,
                "DeleteOptionGroup",
                &[("OptionGroupName", name.as_str())],
            )
            .await,
            400,
            "InvalidOptionGroupStateFault",
        );
        assert_eq!(
            text(&refused, "Message"),
            format!("Default OptionGroup cannot be deleted: {name}"),
            "case {case:?}: expected the default-group refusal naming it, got {refused}"
        );
        expect_ok(&case, describe_option_group(&router, &name).await);
    }
}

/// Every `DBParameterGroupFamily` this service publishes an engine version of, walked over
/// the whole engine-version report page by page.
///
/// The set of default DB parameter groups the account holds is exactly one group per family
/// in this set, so the test derives the expectation from the report rather than writing a
/// list of its own.
async fn every_published_family(router: &MultiServiceRouter) -> std::collections::BTreeSet<String> {
    let mut families = std::collections::BTreeSet::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params = vec![
            ("MaxRecords".to_string(), "100".to_string()),
            ("IncludeAll".to_string(), "true".to_string()),
        ];
        if let Some(marker) = &marker {
            params.push(("Marker".to_string(), marker.clone()));
        }
        let body = expect_ok(
            "every published family",
            call(router, "DescribeDBEngineVersions", &params).await,
        );
        for entry in subtrees(&body, "DBEngineVersion") {
            families.insert(text(&entry, "DBParameterGroupFamily"));
        }
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return families;
        }
    }
}

/// Every default DB parameter group name the account reports, walked page by page.
async fn reported_default_groups(
    router: &MultiServiceRouter,
) -> std::collections::BTreeSet<String> {
    let mut names = std::collections::BTreeSet::new();
    let mut marker: Option<String> = None;
    loop {
        let mut params = vec![("MaxRecords".to_string(), "100".to_string())];
        if let Some(marker) = &marker {
            params.push(("Marker".to_string(), marker.clone()));
        }
        let body = expect_ok(
            "every default group",
            call(router, "DescribeDBParameterGroups", &params).await,
        );
        for name in texts(&body, "DBParameterGroupName") {
            if name.starts_with("default.") {
                names.insert(name);
            }
        }
        marker = optional_text(&body, "Marker");
        if marker.is_none() {
            return names;
        }
    }
}

/// An unfiltered describe reports the account's default groups BESIDE the groups a request
/// created — which is what a caller enumerating the catalog, or resolving a template that
/// names a default group, reads.
#[tokio::test]
async fn an_unfiltered_describe_reports_the_defaults_beside_the_accounts_own_groups() {
    let router = emulator();
    expect_ok(
        "create a parameter group",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "catalog-probe-pg"),
                ("DBParameterGroupFamily", "mysql8.0"),
                ("Description", "a-group-of-the-accounts-own"),
            ],
        )
        .await,
    );
    expect_ok(
        "create an option group",
        call(
            &router,
            "CreateOptionGroup",
            &[
                ("OptionGroupName", "catalog-probe-og"),
                ("EngineName", "mysql"),
                ("MajorEngineVersion", "8.0"),
                ("OptionGroupDescription", "a-group-of-the-accounts-own"),
            ],
        )
        .await,
    );

    let listed = expect_ok(
        "every parameter group",
        call(
            &router,
            "DescribeDBParameterGroups",
            &[("MaxRecords", "100")],
        )
        .await,
    );
    let names = texts(&listed, "DBParameterGroupName");
    assert!(
        names.contains(&"catalog-probe-pg".to_string()),
        "case \"every parameter group\": the account's own group must be reported, got \
         {names:?}"
    );
    // The AWS-managed half is EXACTLY one default group per published family: the catalog
    // the engine-version report publishes decides the set, so a family it loses or gains
    // moves this expectation with it.
    let expected: std::collections::BTreeSet<String> = every_published_family(&router)
        .await
        .iter()
        .map(|family| default_parameter_group_name(family))
        .collect();
    let reported = reported_default_groups(&router).await;
    assert!(
        expected.len() > 1,
        "case \"every parameter group\": the engine-version catalog must publish more than \
         one family, got {expected:?}"
    );
    assert_eq!(
        reported, expected,
        "case \"every parameter group\": the account's default groups must be one per \
         published family"
    );

    let listed = expect_ok(
        "every option group",
        call(&router, "DescribeOptionGroups", &[("MaxRecords", "100")]).await,
    );
    let names = texts(&listed, "OptionGroupName");
    assert!(
        names.contains(&"catalog-probe-og".to_string()),
        "case \"every option group\": the account's own group must be reported, got {names:?}"
    );
    let defaults: Vec<&String> = names
        .iter()
        .filter(|name| name.starts_with("default:"))
        .collect();
    assert!(
        defaults.len() > 1,
        "case \"every option group\": the account's default groups must be reported too, got \
         {names:?}"
    );
    // The catalog spans more than one engine: a default group exists for every engine and
    // major version this service publishes, not for one of them.
    let engines: Vec<String> = texts(&listed, "EngineName");
    let distinct: std::collections::BTreeSet<&String> = engines.iter().collect();
    assert!(
        distinct.len() > 1,
        "case \"every option group\": the catalog must span more than one engine, got \
         {engines:?}"
    );
}

/// A default DB parameter group reports its family's parameters, and a copy of one is
/// refused: "you can't copy a default DB parameter group. Instead, create a new custom DB
/// parameter group, which copies the default parameters and values for the specified DB
/// parameter group family."
#[tokio::test]
async fn a_default_parameter_group_reports_its_familys_parameters_and_cannot_be_copied() {
    let router = emulator();
    let name = "default.mysql8.0";

    let reported = expect_ok(
        "the parameters of a default group",
        call(
            &router,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", name),
                ("MaxRecords", "100"),
            ],
        )
        .await,
    );
    let parameters = subtrees(&reported, "Parameter");
    assert!(
        !parameters.is_empty(),
        "case \"the parameters of a default group\": a default group reports its family's \
         published parameters, got {reported}"
    );
    for parameter in &parameters {
        assert_eq!(
            text(parameter, "Source"),
            "engine-default",
            "case \"the parameters of a default group\": a parameter nobody set reports the \
             engine's own default, got {parameter}"
        );
    }

    let refused = expect_error(
        "a copy of a default group",
        call(
            &router,
            "CopyDBParameterGroup",
            &[
                ("SourceDBParameterGroupIdentifier", name),
                ("TargetDBParameterGroupIdentifier", "copy-of-a-default"),
                ("TargetDBParameterGroupDescription", "not-served"),
            ],
        )
        .await,
        400,
        "InvalidParameterValue",
    );
    assert!(
        text(&refused, "Message").contains(name),
        "case \"a copy of a default group\": the refusal must name the source, got {refused}"
    );
    expect_error(
        "the copy stored nothing",
        describe_parameter_group(&router, "copy-of-a-default").await,
        404,
        "DBParameterGroupNotFound",
    );
}

/// A family publishes more parameters than the smallest page `MaxRecords` admits, so a page
/// of that size is exactly that many records and carries a continuation marker, and
/// resuming from it reports the rest.
#[tokio::test]
async fn a_page_of_a_groups_parameters_is_bounded_and_resumes_from_its_marker() {
    let router = emulator();
    expect_ok(
        "create the group",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "paged-parameters-pg"),
                ("DBParameterGroupFamily", "mysql8.0"),
                ("Description", "a-group-whose-family-publishes-many"),
            ],
        )
        .await,
    );

    // (case, the group the page is taken of) — the account's own group and the default
    // group of the same family, so the rule holds for both kinds.
    for (case, group) in [
        ("a group a request created", "paged-parameters-pg"),
        ("the default group of the family", "default.mysql8.0"),
    ] {
        let whole = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameters",
                &[("DBParameterGroupName", group), ("MaxRecords", "100")],
            )
            .await,
        );
        let total = subtrees(&whole, "Parameter").len();
        assert!(
            total > 20,
            "case {case:?}: the family must publish more parameters than the smallest page, \
             got {total}"
        );

        let first = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameters",
                &[("DBParameterGroupName", group), ("MaxRecords", "20")],
            )
            .await,
        );
        assert_eq!(
            subtrees(&first, "Parameter").len(),
            20,
            "case {case:?}: a page of MaxRecords=20 over more than 20 parameters is exactly \
             20, got {first}"
        );
        let marker = optional_text(&first, "Marker").unwrap_or_else(|| {
            panic!("case {case:?}: a full page must carry a continuation Marker, got {first}")
        });

        let rest = expect_ok(
            case,
            call(
                &router,
                "DescribeDBParameters",
                &[
                    ("DBParameterGroupName", group),
                    ("MaxRecords", "100"),
                    ("Marker", marker.as_str()),
                ],
            )
            .await,
        );
        assert_eq!(
            subtrees(&rest, "Parameter").len(),
            total - 20,
            "case {case:?}: the page after the marker reports the rest of the list, got {rest}"
        );
    }
}
