// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OPTIONS of this service's option groups:
//! `ModifyOptionGroup` adding them and removing them, and the `OptionGroup` envelope
//! `CreateOptionGroup`, `ModifyOptionGroup` and `DescribeOptionGroups` all project.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter`
//! exactly as the production router mounts it (through `db_instances::emulator`, the one
//! mounting helper this test target has), and each request is a real awsQuery form POST
//! carrying the SigV4 credential scope the router keys on. The assertions read the XML
//! the emulator answers with, so a handler that computed nothing could not pass them.
//!
//! The rules under test:
//!
//! * every `OptionGroup` projection reports
//!   `AllowsVpcAndNonVpcInstanceMemberships` — "Indicates whether this option group can
//!   be applied to both VPC and non-VPC instances" — `true` while the group holds no
//!   option and `false` once it holds any, because an option's own placement
//!   requirements bind the group to instances of one network type;
//! * an option a group holds is reported with the facts its engine publishes for it
//!   (description, permanence, persistence, port) and with EVERY setting that option
//!   publishes, each carrying its published default, data type, apply type, allowed
//!   values and modifiability, and its `Value` being the value a request set or, failing
//!   that, that default;
//! * a request naming an option the group's engine does not publish, a setting the
//!   option does not publish, a setting the option publishes as not modifiable, or one
//!   option in both `OptionsToInclude` and `OptionsToRemove`, is refused and leaves the
//!   group exactly as it was.
//!
//! The tables below span what those rules generalise over: EVERY entry of the option
//! catalogue, on every engine that publishes it (the first test walks the catalogue
//! itself, so an entry added to it is covered without touching this file), groups
//! holding zero, one and two options, settings of both published data types, a setting
//! value given in a first and in a second call, and the engines that publish no options
//! at all.

use service_com_amazonaws_rds_amazonrdsv19::state::option_catalog::{OptionFacts, OPTIONS};
use service_com_amazonaws_rds_amazonrdsv19::state::MAX_OPTIONS_PER_REQUEST;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, emulator, expect_error, expect_ok, optional_text, subtrees, text};

/// The query parameters of one call, owned so a table row can hold them.
type Params = Vec<(String, String)>;

/// One `OptionsToInclude` entry: the option name as the request spells it, and the
/// `(setting name, value)` pairs it sets.
type IncludeSpec<'a> = (&'a str, &'a [(&'a str, &'a str)]);

/// A major engine version in the form the engine family writes it — free-form input as
/// far as `CreateOptionGroup` is concerned, so this only keeps the test's groups
/// plausible.
fn major_engine_version(engine_name: &str) -> &'static str {
    if engine_name.starts_with("mariadb") {
        "10.11"
    } else if engine_name.starts_with("mysql") {
        "8.0"
    } else if engine_name.starts_with("oracle") {
        "19"
    } else if engine_name.starts_with("sqlserver") {
        "15.00"
    } else if engine_name.starts_with("db2") {
        "11.5"
    } else {
        "16"
    }
}

/// `CreateOptionGroup` for `engine_name`, asserting the 200 and handing back the body.
async fn create_group(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    engine_name: &str,
) -> String {
    let params: Params = vec![
        ("OptionGroupName".to_string(), name.to_string()),
        ("EngineName".to_string(), engine_name.to_string()),
        (
            "MajorEngineVersion".to_string(),
            major_engine_version(engine_name).to_string(),
        ),
        (
            "OptionGroupDescription".to_string(),
            format!("options-for-{engine_name}"),
        ),
    ];
    expect_ok(case, call(router, "CreateOptionGroup", &params).await)
}

/// The awsQuery parameters of one `ModifyOptionGroup` call.
///
/// The list keys are the ones the protocol builds from each list member's `@xmlName`:
/// `OptionConfigurationList`'s member is named `OptionConfiguration`,
/// `OptionSettingsList`'s is `OptionSetting`, and `OptionNamesList`'s member carries no
/// name of its own, so it takes the Smithy default `member`.
fn modify_params(group: &str, include: &[IncludeSpec<'_>], remove: &[&str]) -> Params {
    let mut params: Params = vec![("OptionGroupName".to_string(), group.to_string())];
    for (index, (option_name, settings)) in include.iter().enumerate() {
        let entry = format!("OptionsToInclude.OptionConfiguration.{}", index + 1);
        params.push((format!("{entry}.OptionName"), (*option_name).to_string()));
        for (setting_index, (setting_name, value)) in settings.iter().enumerate() {
            let setting = format!("{entry}.OptionSettings.OptionSetting.{}", setting_index + 1);
            params.push((format!("{setting}.Name"), (*setting_name).to_string()));
            params.push((format!("{setting}.Value"), (*value).to_string()));
        }
    }
    for (index, option_name) in remove.iter().enumerate() {
        params.push((
            format!("OptionsToRemove.member.{}", index + 1),
            (*option_name).to_string(),
        ));
    }
    params
}

/// `ModifyOptionGroup`, returning the raw status and body so a refusal row can read
/// them.
async fn modify(
    router: &MultiServiceRouter,
    group: &str,
    include: &[IncludeSpec<'_>],
    remove: &[&str],
) -> (u16, String) {
    call(
        router,
        "ModifyOptionGroup",
        &modify_params(group, include, remove),
    )
    .await
}

/// `DescribeOptionGroups` for one group, asserting the 200 and handing back the body.
async fn describe(router: &MultiServiceRouter, case: &str, group: &str) -> String {
    let params: Params = vec![("OptionGroupName".to_string(), group.to_string())];
    expect_ok(case, call(router, "DescribeOptionGroups", &params).await)
}

/// The one `<OptionGroup>` subtree of an answer.
fn the_group(case: &str, xml: &str) -> String {
    let found = subtrees(xml, "OptionGroup");
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <OptionGroup>, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// The `<AllowsVpcAndNonVpcInstanceMemberships>` an answer reports, which must be there.
fn allows_both(case: &str, xml: &str) -> String {
    let group = the_group(case, xml);
    let actual = optional_text(&group, "AllowsVpcAndNonVpcInstanceMemberships");
    actual.unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected an <AllowsVpcAndNonVpcInstanceMemberships> member, \
             got none in {group}"
        )
    })
}

/// Assert `AllowsVpcAndNonVpcInstanceMemberships` is `expected` in this answer.
fn assert_allows_both(case: &str, xml: &str, expected: bool) {
    let actual = allows_both(case, xml);
    assert_eq!(
        actual,
        expected.to_string(),
        "case {case:?}: expected AllowsVpcAndNonVpcInstanceMemberships {expected}, got \
         {actual:?} in {xml}"
    );
}

/// The `<OptionName>` of every option an answer reports for its group, in order.
fn reported_option_names(xml: &str) -> Vec<String> {
    subtrees(xml, "Option")
        .iter()
        .map(|option| text(option, "OptionName"))
        .collect()
}

/// The one `<Option>` subtree named `option_name`.
fn the_option(case: &str, xml: &str, option_name: &str) -> String {
    let found: Vec<String> = subtrees(xml, "Option")
        .into_iter()
        .filter(|option| text(option, "OptionName") == option_name)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <Option> named {option_name}, got {} in {xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// Assert `<tag>` carries `expected` inside `xml`.
fn assert_member(case: &str, xml: &str, tag: &str, expected: &str) {
    let actual = optional_text(xml, tag).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} in {xml}"
    );
}

/// Assert the option subtree reports exactly the settings its catalogue entry
/// publishes, in order, each with its published metadata and with `Value` equal to
/// `set` where that names it and to the published default everywhere else.
fn assert_published_settings(case: &str, option_xml: &str, facts: &OptionFacts, set: &[(&str, &str)]) {
    let settings = subtrees(option_xml, "OptionSetting");
    let reported: Vec<String> = settings
        .iter()
        .map(|setting| text(setting, "Name"))
        .collect();
    let published: Vec<String> = facts
        .settings
        .iter()
        .map(|setting| setting.name.to_string())
        .collect();
    assert_eq!(
        reported, published,
        "case {case:?}: expected the published settings {published:?}, got {reported:?} in \
         {option_xml}"
    );
    for (setting, published) in settings.iter().zip(facts.settings.iter()) {
        let case = format!("{case} setting {}", published.name);
        assert_member(&case, setting, "DefaultValue", published.default_value);
        assert_member(&case, setting, "Description", published.description);
        assert_member(&case, setting, "ApplyType", published.apply_type);
        assert_member(&case, setting, "DataType", published.data_type);
        assert_member(
            &case,
            setting,
            "IsModifiable",
            &published.is_modifiable.to_string(),
        );
        assert_member(
            &case,
            setting,
            "IsCollection",
            &published.is_collection.to_string(),
        );
        // A free-form setting publishes no value list, and then reports no
        // `AllowedValues` member at all.
        let allowed = optional_text(setting, "AllowedValues");
        if published.allowed_values.is_empty() {
            assert!(
                allowed.is_none(),
                "case {case:?}: expected no <AllowedValues>, got {allowed:?} in {setting}"
            );
        } else {
            assert_member(&case, setting, "AllowedValues", published.allowed_values);
        }
        // `Value` is "the current value of the option setting": what a request set, or
        // the published default for a setting nothing has set.
        let expected = set
            .iter()
            .find(|(name, _)| *name == published.name)
            .map_or(published.default_value, |(_, value)| *value);
        assert_member(&case, setting, "Value", expected);
    }
}

/// Assert the option subtree reports the facts its catalogue entry publishes.
fn assert_published_option(case: &str, option_xml: &str, facts: &OptionFacts, set: &[(&str, &str)]) {
    assert_member(case, option_xml, "OptionName", facts.name);
    assert_member(case, option_xml, "OptionDescription", facts.description);
    assert_member(case, option_xml, "Permanent", &facts.permanent.to_string());
    assert_member(case, option_xml, "Persistent", &facts.persistent.to_string());
    // "If required, the port configured for this option to use" — so an option that
    // requires none reports none.
    let port = optional_text(option_xml, "Port");
    match facts.port {
        Some(published) => assert_eq!(
            port,
            Some(published.to_string()),
            "case {case:?}: expected <Port> {published}, got {port:?} in {option_xml}"
        ),
        None => assert!(
            port.is_none(),
            "case {case:?}: expected no <Port>, got {port:?} in {option_xml}"
        ),
    }
    // Nothing in these tests opens an option to a security group, so both membership
    // lists are reported empty rather than filled with invented members.
    for tag in ["DBSecurityGroupMembership", "VpcSecurityGroupMembership"] {
        let members = subtrees(option_xml, tag);
        assert!(
            members.is_empty(),
            "case {case:?}: expected no <{tag}>, got {members:?} in {option_xml}"
        );
    }
    assert_published_settings(case, option_xml, facts, set);
}

#[tokio::test]
async fn every_catalogue_option_reports_its_published_facts_and_settings() {
    // The whole catalogue, on every engine that publishes each entry: a fresh emulator
    // per row, so one engine's group never competes with another's for the per-region
    // option group quota.
    for facts in OPTIONS {
        for engine_name in facts.engines {
            let case = format!("{engine_name}/{}", facts.name);
            let router = emulator();
            // An option group name takes letters, numbers and hyphens only, so the
            // option's name contributes its spelling with the underscores folded out.
            let group = format!(
                "catalog-{}",
                facts.name.to_ascii_lowercase().replace('_', "-")
            );

            // A group nothing has added an option to can be applied to both VPC and
            // non-VPC instances, and reports no option.
            let created = create_group(&router, &case, &group, engine_name).await;
            assert_allows_both(&case, &created, true);
            assert!(
                reported_option_names(&created).is_empty(),
                "case {case:?}: a created group reported options: {created}"
            );

            let modified = expect_ok(
                &case,
                modify(&router, &group, &[(facts.name, &[])], &[]).await,
            );
            // The modify's own answer and the group read back through
            // `DescribeOptionGroups`: the round trip is what proves the option was
            // PERSISTED rather than echoed.
            let described = describe(&router, &case, &group).await;
            for (label, xml) in [("ModifyOptionGroup", &modified), ("DescribeOptionGroups", &described)]
            {
                let case = format!("{case} via {label}");
                assert_allows_both(&case, xml, false);
                let names = reported_option_names(xml);
                assert_eq!(
                    names,
                    vec![facts.name.to_string()],
                    "case {case:?}: expected the group to hold {}, got {names:?} in {xml}",
                    facts.name
                );
                let option = the_option(&case, xml, facts.name);
                assert_published_option(&case, &option, facts, &[]);
            }
        }
    }
}

/// One row of the membership-flag table: the case, the engine, and the options added to
/// one group in order.
struct FlagCase {
    case: &'static str,
    engine_name: &'static str,
    options: &'static [&'static str],
}

/// Groups holding zero, one and two options, across four engine families — what the
/// `AllowsVpcAndNonVpcInstanceMemberships` rule generalises over. `postgres` publishes
/// no option-group options at all, so its group can only ever be empty.
const FLAG_CASES: &[FlagCase] = &[
    FlagCase {
        case: "postgres group, no option to add",
        engine_name: "postgres",
        options: &[],
    },
    FlagCase {
        case: "mariadb group, one option",
        engine_name: "mariadb",
        options: &["MARIADB_AUDIT_PLUGIN"],
    },
    FlagCase {
        case: "mysql group, two options",
        engine_name: "mysql",
        options: &["MARIADB_AUDIT_PLUGIN", "MEMCACHED"],
    },
    FlagCase {
        case: "oracle group, two options",
        engine_name: "oracle-se2",
        options: &["NATIVE_NETWORK_ENCRYPTION", "STATSPACK"],
    },
    FlagCase {
        case: "sqlserver group, one option",
        engine_name: "sqlserver-ee",
        options: &["SQLSERVER_BACKUP_RESTORE"],
    },
];

#[tokio::test]
async fn allows_both_memberships_follows_whether_the_group_holds_an_option() {
    for row in FLAG_CASES {
        let router = emulator();
        let group = "flag-group";
        let created = create_group(&router, row.case, group, row.engine_name).await;
        assert_allows_both(row.case, &created, true);
        assert_allows_both(
            row.case,
            &describe(&router, row.case, group).await,
            true,
        );

        // Adding options, one call at a time: the flag turns false on the FIRST option
        // and stays false while any remains.
        for (index, option_name) in row.options.iter().enumerate() {
            let case = format!("{} after adding {option_name}", row.case);
            let modified = expect_ok(&case, modify(&router, group, &[(option_name, &[])], &[]).await);
            assert_allows_both(&case, &modified, false);
            assert_allows_both(&case, &describe(&router, &case, group).await, false);
            let names = reported_option_names(&modified);
            assert_eq!(
                names.len(),
                index + 1,
                "case {case:?}: expected {} options, got {names:?} in {modified}",
                index + 1
            );
        }

        // Removing them again, one at a time: the flag turns true only when the LAST
        // option is gone.
        for (index, option_name) in row.options.iter().enumerate() {
            let case = format!("{} after removing {option_name}", row.case);
            let modified = expect_ok(&case, modify(&router, group, &[], &[option_name]).await);
            let remaining = row.options.len() - index - 1;
            assert_allows_both(&case, &modified, remaining == 0);
            assert_allows_both(&case, &describe(&router, &case, group).await, remaining == 0);
            let names = reported_option_names(&modified);
            assert_eq!(
                names.len(),
                remaining,
                "case {case:?}: expected {remaining} options, got {names:?} in {modified}"
            );
        }
    }
}

/// One row of the settings table: the case, the engine, the option, and the settings the
/// request sets on it.
struct SettingCase {
    case: &'static str,
    engine_name: &'static str,
    option_name: &'static str,
    set: &'static [(&'static str, &'static str)],
}

/// Settings set across the catalogue: a collection setting and a numeric one of the
/// MariaDB audit plugin, a numeric memcached setting, an Oracle encryption setting, and
/// two SQL Server audit settings at once — spanning both published data types, both
/// `IsCollection` values and one and two settings per call.
const SETTING_CASES: &[SettingCase] = &[
    SettingCase {
        case: "mariadb audit events",
        engine_name: "mariadb",
        option_name: "MARIADB_AUDIT_PLUGIN",
        set: &[("SERVER_AUDIT_EVENTS", "CONNECT,QUERY,TABLE")],
    },
    SettingCase {
        case: "mariadb rotation count",
        engine_name: "mariadb",
        option_name: "MARIADB_AUDIT_PLUGIN",
        set: &[("SERVER_AUDIT_FILE_ROTATIONS", "20")],
    },
    SettingCase {
        case: "mysql memcached isolation level",
        engine_name: "mysql",
        option_name: "MEMCACHED",
        set: &[("INNODB_API_TRX_LEVEL", "3")],
    },
    SettingCase {
        case: "oracle encryption server",
        engine_name: "oracle-ee",
        option_name: "NATIVE_NETWORK_ENCRYPTION",
        set: &[
            ("SQLNET.ENCRYPTION_SERVER", "REQUIRED"),
            ("SQLNET.ENCRYPTION_TYPES_SERVER", "AES256"),
        ],
    },
    SettingCase {
        case: "sqlserver audit retention and compression",
        engine_name: "sqlserver-se",
        option_name: "SQLSERVER_AUDIT",
        set: &[("RETENTION_TIME", "24"), ("ENABLE_COMPRESSION", "false")],
    },
];

#[tokio::test]
async fn a_setting_a_request_sets_replaces_its_value_and_keeps_its_default() {
    for row in SETTING_CASES {
        let router = emulator();
        let group = "setting-group";
        create_group(&router, row.case, group, row.engine_name).await;
        let modified = expect_ok(
            row.case,
            modify(&router, group, &[(row.option_name, row.set)], &[]).await,
        );
        let described = describe(&router, row.case, group).await;
        let facts = OPTIONS
            .iter()
            .find(|facts| facts.name == row.option_name)
            .expect("the table names catalogue options");
        for (label, xml) in [("ModifyOptionGroup", &modified), ("DescribeOptionGroups", &described)]
        {
            let case = format!("{} via {label}", row.case);
            let option = the_option(&case, xml, row.option_name);
            assert_published_option(&case, &option, facts, row.set);
        }
    }
}

#[tokio::test]
async fn a_second_call_updates_the_configuration_and_keeps_what_the_first_set() {
    let router = emulator();
    let group = "accumulate-group";
    create_group(&router, "accumulate", group, "mariadb").await;

    // "Options in this list are added to the option group or, if already present, the
    // specified configuration is used to update the existing configuration."
    expect_ok(
        "first call",
        modify(
            &router,
            group,
            &[(
                "MARIADB_AUDIT_PLUGIN",
                &[("SERVER_AUDIT_EVENTS", "CONNECT,QUERY,TABLE")],
            )],
            &[],
        )
        .await,
    );
    let second = expect_ok(
        "second call",
        modify(
            &router,
            group,
            &[(
                "MARIADB_AUDIT_PLUGIN",
                &[("SERVER_AUDIT_FILE_ROTATIONS", "20")],
            )],
            &[],
        )
        .await,
    );
    let facts = OPTIONS
        .iter()
        .find(|facts| facts.name == "MARIADB_AUDIT_PLUGIN")
        .expect("the catalogue publishes the MariaDB audit plugin");
    let both = [
        ("SERVER_AUDIT_EVENTS", "CONNECT,QUERY,TABLE"),
        ("SERVER_AUDIT_FILE_ROTATIONS", "20"),
    ];
    for (label, xml) in [
        ("ModifyOptionGroup", second.clone()),
        (
            "DescribeOptionGroups",
            describe(&router, "second call", group).await,
        ),
    ] {
        let case = format!("both settings via {label}");
        let option = the_option(&case, &xml, "MARIADB_AUDIT_PLUGIN");
        assert_published_option(&case, &option, facts, &both);
    }

    // The group still holds ONE option, not one per call.
    let names = reported_option_names(&second);
    assert_eq!(
        names,
        vec!["MARIADB_AUDIT_PLUGIN".to_string()],
        "case \"one option after two calls\": got {names:?} in {second}"
    );

    // An option name is a published identifier, so the catalogue's own spelling is what
    // the group reports however the request spelled it.
    let lowercase = expect_ok(
        "lowercase spelling",
        modify(
            &router,
            group,
            &[("mariadb_audit_plugin", &[("SERVER_AUDIT_FILE_ROTATIONS", "21")])],
            &[],
        )
        .await,
    );
    let names = reported_option_names(&lowercase);
    assert_eq!(
        names,
        vec!["MARIADB_AUDIT_PLUGIN".to_string()],
        "case \"lowercase spelling\": got {names:?} in {lowercase}"
    );
}

#[tokio::test]
async fn removing_an_option_the_group_does_not_hold_changes_nothing() {
    let router = emulator();
    let group = "idempotent-group";
    create_group(&router, "idempotent removal", group, "mysql").await;
    expect_ok(
        "add memcached",
        modify(&router, group, &[("MEMCACHED", &[])], &[]).await,
    );

    // "Options in this list are removed from the option group": a name the group does
    // not hold leaves it as the request asks for it, so the call succeeds and changes
    // nothing.
    let modified = expect_ok(
        "remove an option the group never held",
        modify(&router, group, &[], &["MARIADB_AUDIT_PLUGIN"]).await,
    );
    let names = reported_option_names(&modified);
    assert_eq!(
        names,
        vec!["MEMCACHED".to_string()],
        "case \"remove an option the group never held\": got {names:?} in {modified}"
    );
    assert_allows_both("remove an option the group never held", &modified, false);
}

#[tokio::test]
async fn a_request_past_the_option_and_settings_bounds_is_refused() {
    let router = emulator();
    let group = "bounded-group";
    create_group(&router, "bounds", group, "mariadb").await;

    // The lists one request may carry are capped before any per-entry work runs, so
    // each is exercised AT the bound and one entry past it. `OptionsToRemove` names
    // options the group does not hold, which is a no-op at the bound — so a refusal
    // there could only come from the bound itself.
    let removals: Vec<String> = (0..=MAX_OPTIONS_PER_REQUEST)
        .map(|index| format!("OPTION-{index}"))
        .collect();
    let at_bound: Vec<&str> = removals[..MAX_OPTIONS_PER_REQUEST]
        .iter()
        .map(String::as_str)
        .collect();
    let past_bound: Vec<&str> = removals.iter().map(String::as_str).collect();
    expect_ok(
        "OptionsToRemove at the bound",
        modify(&router, group, &[], &at_bound).await,
    );
    expect_error(
        "OptionsToRemove one past the bound",
        modify(&router, group, &[], &past_bound).await,
        400,
        "ValidationException",
    );

    // The same bound on one option configuration's settings: the same published setting
    // named repeatedly, so a refusal can only come from the count.
    let settings: Vec<(&str, &str)> = (0..=MAX_OPTIONS_PER_REQUEST)
        .map(|_| ("SERVER_AUDIT_FILE_ROTATIONS", "20"))
        .collect();
    expect_ok(
        "OptionSettings at the bound",
        modify(
            &router,
            group,
            &[(
                "MARIADB_AUDIT_PLUGIN",
                &settings[..MAX_OPTIONS_PER_REQUEST],
            )],
            &[],
        )
        .await,
    );
    expect_error(
        "OptionSettings one past the bound",
        modify(&router, group, &[("MARIADB_AUDIT_PLUGIN", &settings)], &[]).await,
        400,
        "ValidationException",
    );
}

/// One row of the refusal table: the case, the options to include, the options to
/// remove, and the HTTP status and wire error code the request must be refused with.
struct RefusalCase {
    case: &'static str,
    include: &'static [IncludeSpec<'static>],
    remove: &'static [&'static str],
    status: u16,
    code: &'static str,
}

/// The requests this service refuses, differing along what each rule generalises over:
/// an option another engine publishes, an option no engine publishes, a setting the
/// option does not publish, a setting published as not modifiable, and one option named
/// in both lists.
const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "an option another engine publishes",
        include: &[("MEMCACHED", &[])],
        remove: &[],
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an option no engine publishes",
        include: &[("NO_SUCH_OPTION", &[])],
        remove: &[],
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "a setting the option does not publish",
        include: &[("MARIADB_AUDIT_PLUGIN", &[("SERVER_AUDIT_NOPE", "1")])],
        remove: &[],
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "a setting published as not modifiable",
        include: &[("MARIADB_AUDIT_PLUGIN", &[("SERVER_AUDIT_LOGGING", "OFF")])],
        remove: &[],
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "one option in both lists",
        include: &[("MARIADB_AUDIT_PLUGIN", &[])],
        remove: &["MARIADB_AUDIT_PLUGIN"],
        status: 400,
        code: "ValidationException",
    },
];

#[tokio::test]
async fn a_request_this_service_refuses_leaves_the_group_as_it_was() {
    let router = emulator();
    let group = "refusal-group";
    create_group(&router, "refusals", group, "mariadb").await;

    for row in REFUSAL_CASES {
        expect_error(
            row.case,
            modify(&router, group, row.include, row.remove).await,
            row.status,
            row.code,
        );
        // Nothing was written: the group still holds no option, so it can still be
        // applied to both VPC and non-VPC instances.
        let described = describe(&router, row.case, group).await;
        let names = reported_option_names(&described);
        assert!(
            names.is_empty(),
            "case {:?}: a refused request changed the group: {names:?} in {described}",
            row.case
        );
        assert_allows_both(row.case, &described, true);
    }

    // The same rows, now against a group that already holds an option: a refusal must
    // not drop what the group holds either.
    expect_ok(
        "seed an option",
        modify(&router, group, &[("MARIADB_AUDIT_PLUGIN", &[])], &[]).await,
    );
    for row in REFUSAL_CASES {
        // "one option in both lists" is the only row that can still be refused once the
        // option is present; the others are refused for the same reason as before.
        expect_error(
            row.case,
            modify(&router, group, row.include, row.remove).await,
            row.status,
            row.code,
        );
        let described = describe(&router, row.case, group).await;
        let names = reported_option_names(&described);
        assert_eq!(
            names,
            vec!["MARIADB_AUDIT_PLUGIN".to_string()],
            "case {:?}: a refused request changed the group: {names:?} in {described}",
            row.case
        );
    }
}

#[tokio::test]
async fn a_permanent_option_cannot_be_removed_and_a_missing_group_is_the_modelled_404() {
    let router = emulator();
    let group = "permanent-group";
    create_group(&router, "permanent option", group, "sqlserver-ee").await;

    // "Permanent options, such as the TDE option for Oracle Advanced Security TDE,
    // can't be removed from an option group."
    let added = expect_ok(
        "add the permanent option",
        modify(&router, group, &[("TDE", &[])], &[]).await,
    );
    let option = the_option("add the permanent option", &added, "TDE");
    assert_member("add the permanent option", &option, "Permanent", "true");
    expect_error(
        "remove the permanent option",
        modify(&router, group, &[], &["TDE"]).await,
        400,
        "ValidationException",
    );
    let described = describe(&router, "remove the permanent option", group).await;
    let names = reported_option_names(&described);
    assert_eq!(
        names,
        vec!["TDE".to_string()],
        "case \"remove the permanent option\": got {names:?} in {described}"
    );

    // A name no stored group has is the modelled 404, not a group this call creates.
    let body = expect_error(
        "modify a group that does not exist",
        modify(&router, "no-such-group", &[("TDE", &[])], &[]).await,
        404,
        "OptionGroupNotFoundFault",
    );
    let message = text(&body, "Message");
    assert!(
        message.contains("no-such-group"),
        "case \"modify a group that does not exist\": expected the message to name the \
         group, got {message:?}"
    );
}
