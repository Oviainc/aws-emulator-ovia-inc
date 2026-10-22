// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that PIN the option-setting definitions this service publishes for
//! an option-group option, independently of the catalogue constants the emulator reads.
//!
//! `tests/all/option_group_options.rs` asserts the shape of the projection against
//! `crate::state::option_catalog` itself, so it cannot tell whether a row of that
//! catalogue carries the values the published option table does. The table below
//! restates those values LITERALLY — one row per published setting — and asserts them
//! over the wire, so a row whose name, default, apply type, data type, allowed-value
//! list, modifiability or collection flag drifts from the published definition fails
//! here.
//!
//! The rule under test: a group reports, for each option it holds, EVERY setting the
//! option's published table defines, each with that table's name, description-bearing
//! metadata (apply type, data type), its published default, its published allowed-value
//! list (and no `AllowedValues` member at all where the table publishes no list), and
//! its published modifiability — with `Value` being what a request set on a modifiable
//! setting and the published default everywhere else, on both the mutation echo and the
//! `DescribeOptionGroups` read-back.
//!
//! The rows span what the rule generalises over: four options on four engine families
//! (and one option on BOTH engines that publish it), settings of both published data
//! types, allowed-value lists that are a value enumeration, a `low-high` range and
//! absent, defaults that are empty and non-empty, settings published as modifiable and
//! as fixed, and settings published as collections and as single-valued. The MariaDB
//! audit plugin's nine settings are the class the option-settings issue was filed
//! about; the memcached, Oracle network-encryption and SQL Server audit rows are
//! entries that issue did not cite.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{emulator, expect_error, expect_ok, optional_text, subtrees, text};
use crate::option_group_options::{create_group, describe, modify, the_option};

/// One published setting of one option: everything the published table defines for it,
/// plus a value inside its published allowed-value list for the rows a request may set.
struct PublishedSetting {
    /// `OptionSetting$Name`.
    name: &'static str,
    /// `OptionSetting$DefaultValue`.
    default_value: &'static str,
    /// `OptionSetting$AllowedValues`, EMPTY where the table publishes no list, which is
    /// then reported with no `AllowedValues` member at all.
    allowed_values: &'static str,
    /// `OptionSetting$ApplyType`.
    apply_type: &'static str,
    /// `OptionSetting$DataType`.
    data_type: &'static str,
    /// `OptionSetting$IsModifiable`.
    is_modifiable: bool,
    /// `OptionSetting$IsCollection`.
    is_collection: bool,
    /// A value within the published allowed-value list, different from the default —
    /// what the modifiable rows are set to. Unused for a fixed row.
    settable: &'static str,
}

/// One published option: the engines that publish it and its settings, in the order the
/// group reports them.
struct PublishedOption {
    engine_name: &'static str,
    option_name: &'static str,
    settings: &'static [PublishedSetting],
}

/// The MariaDB Audit Plugin's NINE published settings.
///
/// `SERVER_AUDIT` is the plugin-activation setting: published with the single allowed
/// value `FORCE_PLUS_PERMANENT` and as not modifiable, so it always reports that value.
/// `SERVER_AUDIT_LOGGING` is published with the single allowed value `ON` and
/// `SERVER_AUDIT_FILE_PATH` with the RDS audit-log directory, both fixed as well: RDS
/// loads the plugin with logging on and writes its files where RDS chooses.
const MARIADB_AUDIT_PLUGIN_SETTINGS: &[PublishedSetting] = &[
    PublishedSetting {
        name: "SERVER_AUDIT_LOGGING",
        default_value: "ON",
        allowed_values: "ON",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: false,
        is_collection: false,
        settable: "",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_INCL_USERS",
        default_value: "",
        allowed_values: "",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: true,
        settable: "auditor",
    },
    PublishedSetting {
        name: "SERVER_AUDIT",
        default_value: "FORCE_PLUS_PERMANENT",
        allowed_values: "FORCE_PLUS_PERMANENT",
        apply_type: "STATIC",
        data_type: "STRING",
        is_modifiable: false,
        is_collection: false,
        settable: "",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_QUERY_LOG_LIMIT",
        default_value: "1024",
        allowed_values: "0-2147483647",
        apply_type: "DYNAMIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "2048",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_FILE_ROTATE_SIZE",
        default_value: "1000000",
        allowed_values: "1-1000000000",
        apply_type: "DYNAMIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        // The published upper bound of the range.
        settable: "1000000000",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_FILE_PATH",
        default_value: "/rdsdbdata/log/audit/",
        allowed_values: "",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: false,
        is_collection: false,
        settable: "",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_EVENTS",
        default_value: "CONNECT,QUERY",
        allowed_values: "CONNECT,QUERY,QUERY_DDL,QUERY_DML,QUERY_DCL,QUERY_DML_NO_SELECT",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: true,
        settable: "CONNECT,QUERY,QUERY_DDL",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_EXCL_USERS",
        default_value: "",
        allowed_values: "",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: true,
        settable: "rdsadmin",
    },
    PublishedSetting {
        name: "SERVER_AUDIT_FILE_ROTATIONS",
        default_value: "9",
        allowed_values: "0-100",
        apply_type: "DYNAMIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        // The published upper bound of the range.
        settable: "100",
    },
];

/// memcached's published settings — an option the option-settings issue did not cite,
/// and the one option that publishes a port of its own.
const MEMCACHED_SETTINGS: &[PublishedSetting] = &[
    PublishedSetting {
        name: "DAEMON_MEMCACHED_R_BATCH_SIZE",
        default_value: "1",
        allowed_values: "1-1073741824",
        apply_type: "STATIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "32",
    },
    PublishedSetting {
        name: "DAEMON_MEMCACHED_W_BATCH_SIZE",
        default_value: "1",
        allowed_values: "1-1073741824",
        apply_type: "STATIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "1073741824",
    },
    PublishedSetting {
        name: "DAEMON_MEMCACHED_OPTION",
        default_value: "",
        allowed_values: "",
        apply_type: "STATIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "-v",
    },
    PublishedSetting {
        name: "INNODB_API_DISABLE_ROWLOCK",
        default_value: "0",
        allowed_values: "0,1",
        apply_type: "STATIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "1",
    },
    PublishedSetting {
        name: "INNODB_API_ENABLE_MDL",
        default_value: "0",
        allowed_values: "0,1",
        apply_type: "STATIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "1",
    },
    PublishedSetting {
        name: "INNODB_API_TRX_LEVEL",
        default_value: "0",
        allowed_values: "0-3",
        apply_type: "STATIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        settable: "3",
    },
];

/// Oracle native network encryption's published settings — the option the modelled
/// `OptionSetting` shape cites itself, and another entry the issue did not cite.
const NATIVE_NETWORK_ENCRYPTION_SETTINGS: &[PublishedSetting] = &[
    PublishedSetting {
        name: "SQLNET.ENCRYPTION_SERVER",
        default_value: "REQUESTED",
        allowed_values: "ACCEPTED,REJECTED,REQUESTED,REQUIRED",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "REQUIRED",
    },
    PublishedSetting {
        name: "SQLNET.ENCRYPTION_TYPES_SERVER",
        default_value: "AES256,AES192,AES128",
        allowed_values: "AES256,AES192,AES128,3DES168,3DES112,DES,DES40,RC4_256,RC4_128,RC4_56,RC4_40",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: true,
        settable: "AES256",
    },
    PublishedSetting {
        name: "SQLNET.CRYPTO_CHECKSUM_SERVER",
        default_value: "REQUESTED",
        allowed_values: "ACCEPTED,REJECTED,REQUESTED,REQUIRED",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "ACCEPTED",
    },
    PublishedSetting {
        name: "SQLNET.CRYPTO_CHECKSUM_TYPES_SERVER",
        default_value: "SHA1",
        allowed_values: "SHA1,MD5",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: true,
        settable: "MD5",
    },
];

/// SQL Server audit's published settings — a fourth entry the issue did not cite, whose
/// free-form rows take an ARN rather than a value from a list.
const SQLSERVER_AUDIT_SETTINGS: &[PublishedSetting] = &[
    PublishedSetting {
        name: "IAM_ROLE_ARN",
        default_value: "",
        allowed_values: "",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "arn:aws:iam::000000000000:role/rds-audit",
    },
    PublishedSetting {
        name: "S3_BUCKET_ARN",
        default_value: "",
        allowed_values: "",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "arn:aws:s3:::rds-audit-logs",
    },
    PublishedSetting {
        name: "ENABLE_COMPRESSION",
        default_value: "true",
        allowed_values: "true,false",
        apply_type: "DYNAMIC",
        data_type: "STRING",
        is_modifiable: true,
        is_collection: false,
        settable: "false",
    },
    PublishedSetting {
        name: "RETENTION_TIME",
        default_value: "0",
        allowed_values: "0-840",
        apply_type: "DYNAMIC",
        data_type: "INTEGER",
        is_modifiable: true,
        is_collection: false,
        // The published upper bound of the range.
        settable: "840",
    },
];

/// The published options this suite pins, including the audit plugin on BOTH engines
/// that publish it: the same option on another engine is the same published table.
const PUBLISHED_OPTIONS: &[PublishedOption] = &[
    PublishedOption {
        engine_name: "mariadb",
        option_name: "MARIADB_AUDIT_PLUGIN",
        settings: MARIADB_AUDIT_PLUGIN_SETTINGS,
    },
    PublishedOption {
        engine_name: "mysql",
        option_name: "MARIADB_AUDIT_PLUGIN",
        settings: MARIADB_AUDIT_PLUGIN_SETTINGS,
    },
    PublishedOption {
        engine_name: "mysql",
        option_name: "MEMCACHED",
        settings: MEMCACHED_SETTINGS,
    },
    PublishedOption {
        engine_name: "oracle-ee",
        option_name: "NATIVE_NETWORK_ENCRYPTION",
        settings: NATIVE_NETWORK_ENCRYPTION_SETTINGS,
    },
    PublishedOption {
        engine_name: "sqlserver-se",
        option_name: "SQLSERVER_AUDIT",
        settings: SQLSERVER_AUDIT_SETTINGS,
    },
];

/// The `<OptionSetting>` subtree named `name`, which must be reported exactly once.
fn the_setting(case: &str, option_xml: &str, name: &str) -> String {
    let found: Vec<String> = subtrees(option_xml, "OptionSetting")
        .into_iter()
        .filter(|setting| text(setting, "Name") == name)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <OptionSetting> named {name}, got {} in \
         {option_xml}",
        found.len()
    );
    found.into_iter().next().expect("checked just above")
}

/// Assert `<tag>` inside `xml` carries `expected` — an absent member reading as empty,
/// which is how a published empty default is reported.
fn assert_setting_member(case: &str, xml: &str, tag: &str, expected: &str) {
    let actual = optional_text(xml, tag).unwrap_or_default();
    assert_eq!(
        actual, expected,
        "case {case:?}: expected <{tag}> {expected:?}, got {actual:?} in {xml}"
    );
}

/// Assert one setting subtree reports the published definition, with `Value` equal to
/// `expected_value`.
fn assert_published_definition(
    case: &str,
    option_xml: &str,
    published: &PublishedSetting,
    expected_value: &str,
) {
    let case = format!("{case} setting {}", published.name);
    let setting = the_setting(&case, option_xml, published.name);
    assert_setting_member(&case, &setting, "DefaultValue", published.default_value);
    assert_setting_member(&case, &setting, "ApplyType", published.apply_type);
    assert_setting_member(&case, &setting, "DataType", published.data_type);
    assert_setting_member(
        &case,
        &setting,
        "IsModifiable",
        &published.is_modifiable.to_string(),
    );
    assert_setting_member(
        &case,
        &setting,
        "IsCollection",
        &published.is_collection.to_string(),
    );
    // A setting the table publishes no value list for reports no `AllowedValues` member
    // at all, not an empty one.
    let allowed = optional_text(&setting, "AllowedValues");
    if published.allowed_values.is_empty() {
        assert!(
            allowed.is_none(),
            "case {case:?}: expected no <AllowedValues>, got {allowed:?} in {setting}"
        );
    } else {
        assert_setting_member(&case, &setting, "AllowedValues", published.allowed_values);
    }
    // Every setting carries a description: it is what a client reads to learn what the
    // setting does.
    let description = optional_text(&setting, "Description").unwrap_or_default();
    assert!(
        !description.is_empty(),
        "case {case:?}: expected a non-empty <Description>, got {description:?} in {setting}"
    );
    assert_setting_member(&case, &setting, "Value", expected_value);
}

/// The `<Name>` of every setting an option subtree reports, in the order reported.
fn reported_setting_names(option_xml: &str) -> Vec<String> {
    subtrees(option_xml, "OptionSetting")
        .iter()
        .map(|setting| text(setting, "Name"))
        .collect()
}

/// `(setting name, value)` pairs for every modifiable row of `option`.
fn settable_pairs(option: &PublishedOption) -> Vec<(&'static str, &'static str)> {
    option
        .settings
        .iter()
        .filter(|published| published.is_modifiable)
        .map(|published| (published.name, published.settable))
        .collect()
}

/// A group name for one row: letters, numbers and hyphens only, which is what
/// `CreateOptionGroup` admits.
fn group_name(option: &PublishedOption) -> String {
    format!(
        "pinned-{}-{}",
        option.engine_name.replace('_', "-"),
        option.option_name.to_ascii_lowercase().replace(['_', '.'], "-")
    )
}

/// Add `option` to a fresh group and hand back the mutation echo and the read-back, so
/// every assertion runs against both paths.
async fn add_and_read_back(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    option: &PublishedOption,
    set: &[(&str, &str)],
) -> Vec<(String, String)> {
    let modified = expect_ok(case, modify(router, group, &[(option.option_name, set)], &[]).await);
    let described = describe(router, case, group).await;
    [("ModifyOptionGroup", modified), ("DescribeOptionGroups", described)]
        .into_iter()
        .map(|(label, xml)| (label.to_string(), xml))
        .collect()
}

#[tokio::test]
async fn an_option_reports_every_setting_its_published_table_defines() {
    for option in PUBLISHED_OPTIONS {
        let case = format!("{}/{}", option.engine_name, option.option_name);
        // A fresh emulator per row, so one engine's group never competes with another's
        // for the per-region option-group quota.
        let router = emulator();
        let group = group_name(option);
        create_group(&router, &case, &group, option.engine_name).await;

        for (label, xml) in add_and_read_back(&router, &case, &group, option, &[]).await {
            let case = format!("{case} via {label}");
            let option_xml = the_option(&case, &xml, option.option_name);
            // Exactly the published settings, each exactly once: a table missing one of
            // them (or carrying a name the table does not define) fails here.
            let reported = reported_setting_names(&option_xml);
            let published: Vec<String> = option
                .settings
                .iter()
                .map(|setting| setting.name.to_string())
                .collect();
            let mut sorted_reported = reported.clone();
            sorted_reported.sort();
            let mut sorted_published = published.clone();
            sorted_published.sort();
            assert_eq!(
                sorted_reported, sorted_published,
                "case {case:?}: expected the published settings {published:?}, got \
                 {reported:?} in {option_xml}"
            );
            // Nothing has set a value, so every setting reports its published default.
            for published in option.settings {
                assert_published_definition(
                    &case,
                    &option_xml,
                    published,
                    published.default_value,
                );
            }
        }
    }
}

#[tokio::test]
async fn a_set_value_replaces_the_default_only_for_the_settings_a_request_set() {
    for option in PUBLISHED_OPTIONS {
        let case = format!("{}/{}", option.engine_name, option.option_name);
        let router = emulator();
        let group = group_name(option);
        create_group(&router, &case, &group, option.engine_name).await;

        let set = settable_pairs(option);
        for (label, xml) in add_and_read_back(&router, &case, &group, option, &set).await {
            let case = format!("{case} with every modifiable setting set via {label}");
            let option_xml = the_option(&case, &xml, option.option_name);
            for published in option.settings {
                // A modifiable setting reports what the request set while
                // `DefaultValue` keeps reporting the published default; a fixed one
                // keeps reporting that default as its value too.
                let expected_value = if published.is_modifiable {
                    published.settable
                } else {
                    published.default_value
                };
                assert_published_definition(&case, &option_xml, published, expected_value);
            }
        }
    }
}

#[tokio::test]
async fn a_setting_published_as_fixed_is_refused_and_keeps_its_published_value() {
    for option in PUBLISHED_OPTIONS {
        let fixed: Vec<&PublishedSetting> = option
            .settings
            .iter()
            .filter(|published| !published.is_modifiable)
            .collect();
        if fixed.is_empty() {
            continue;
        }
        let case = format!("{}/{}", option.engine_name, option.option_name);
        let router = emulator();
        let group = group_name(option);
        create_group(&router, &case, &group, option.engine_name).await;
        expect_ok(
            &case,
            modify(&router, &group, &[(option.option_name, &[])], &[]).await,
        );

        for published in fixed {
            let case = format!("{case} fixed setting {}", published.name);
            // "Indicates whether the option setting can be modified from the default" —
            // a request that sets one of these is refused, whatever value it names.
            for value in ["OFF", published.default_value] {
                expect_error(
                    &case,
                    modify(
                        &router,
                        &group,
                        &[(option.option_name, &[(published.name, value)])],
                        &[],
                    )
                    .await,
                    400,
                    "ValidationException",
                );
            }
            // The refusal changed nothing: the setting still reports its published
            // value, read back through `DescribeOptionGroups`.
            let described = describe(&router, &case, &group).await;
            let option_xml = the_option(&case, &described, option.option_name);
            assert_published_definition(
                &case,
                &option_xml,
                published,
                published.default_value,
            );
        }
    }
}
