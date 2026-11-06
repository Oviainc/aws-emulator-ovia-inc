// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ResetDBParameterGroup`: what a reset does to the DB (instance)
//! parameter group it names, read back through `DescribeDBParameters`.
//!
//! A group holds only the parameters a CALLER set; the engine's own defaults belong to the
//! FAMILY, which publishes them. So the rule under test is: a reset DROPS the settings the
//! group carries for the parameters its request names — or every setting the group carries
//! when the request names the whole-group scope — leaving every other setting untouched,
//! after which the family's published default is what the group reports for a dropped
//! parameter.
//!
//! The rule is asserted over a TABLE spanning the axes it generalises over, not over one
//! request:
//!
//! * the SCOPE the request names, in each form the two members can carry it:
//!   `ResetAllParameters` true, the member absent together with an absent parameter list
//!   (the scope `ResetAllParameters` documents as its default), a single name, several
//!   names, every held name, a name list beside an explicit `ResetAllParameters` false, and
//!   that explicit false with no name at all — a request for no parameter, served as the
//!   no-op it asks for;
//! * which parameters the group holds: one the family publishes, one it does not, a name
//!   the group holds no setting under at all, and a family that publishes no catalog;
//! * the apply method the dropped change carried — immediate, deferred to the next reboot,
//!   and the one a STATIC parameter implies — because a change still waiting on a reboot
//!   does not block this operation's reset;
//! * the request bound at the documented maximum and one past it;
//! * the refusal classes: a group that is not there, a group Amazon RDS provides itself,
//!   both scopes at once, and a malformed entry;
//! * the regional scope, over two regions and one group name;
//! * idempotence: the same reset twice reports the same group.
//!
//! Every call goes over the wire through the same front door the production router mounts
//! (`crate::db_instances::emulator`), and every assertion reads the XML the emulator
//! answered with.

use std::collections::BTreeMap;

use service_com_amazonaws_rds_amazonrdsv19::state::{
    published_db_parameters, APPLY_METHOD_IMMEDIATE, APPLY_METHOD_PENDING_REBOOT,
    CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE, MAX_PARAMETERS_PER_MODIFY,
    PARAMETER_SOURCE_ENGINE_DEFAULT, PARAMETER_SOURCE_USER,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, call_in, emulator, expect_error, expect_ok, text, REGION};
use crate::db_parameters::{create_group, describe, modify_in};
use crate::engine_default_parameters::reported;

/// A family whose instance-level parameter catalog this front door publishes.
const PUBLISHED_FAMILY: &str = "mysql8.0";

/// A family this front door publishes NO instance-level parameter catalog for, so every
/// setting of a group of this family is a name the catalog does not carry.
const UNPUBLISHED_FAMILY: &str = "db2-se";

/// The `awsQueryError` code of `DBParameterGroupNotFoundFault`.
const NOT_FOUND: &str = "DBParameterGroupNotFound";

/// The Query API code Amazon RDS answers a parameter value it cannot serve with.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The Query API code Amazon RDS answers members that cannot hold together with.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// One setting a row writes into its group before the reset: the parameter, its value, and
/// the apply method where the request names one.
type Setting = (&'static str, &'static str, Option<&'static str>);

/// The request members a reset carries beyond `DBParameterGroupName`: one
/// `Parameters.Parameter.<n>.ParameterName` per name, plus `ResetAllParameters` where the
/// request names it.
fn reset_members(names: &[&str], reset_all: Option<bool>) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = Vec::new();
    if let Some(reset_all) = reset_all {
        params.push(("ResetAllParameters".to_string(), reset_all.to_string()));
    }
    for (index, name) in names.iter().enumerate() {
        params.push((
            format!("Parameters.Parameter.{}.ParameterName", index + 1),
            (*name).to_string(),
        ));
    }
    params
}

/// `ResetDBParameterGroup` on `group` in `region`.
async fn reset_in(
    router: &MultiServiceRouter,
    region: &str,
    group: &str,
    names: &[&str],
    reset_all: Option<bool>,
) -> (u16, String) {
    let mut params: Vec<(String, String)> =
        vec![("DBParameterGroupName".to_string(), group.to_string())];
    params.extend(reset_members(names, reset_all));
    call_in(router, region, "ResetDBParameterGroup", &params).await
}

/// [`reset_in`] against [`REGION`].
async fn reset(
    router: &MultiServiceRouter,
    group: &str,
    names: &[&str],
    reset_all: Option<bool>,
) -> (u16, String) {
    reset_in(router, REGION, group, names, reset_all).await
}

/// The settings `group` holds of its own, by parameter name: what `Source` `user` reports,
/// which is exactly the parameters a caller set.
async fn held_in(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    group: &str,
) -> BTreeMap<String, Option<String>> {
    let body = expect_ok(
        case,
        call_in(
            router,
            region,
            "DescribeDBParameters",
            &[
                ("DBParameterGroupName", group),
                ("Source", PARAMETER_SOURCE_USER),
            ],
        )
        .await,
    );
    reported(&body)
        .into_iter()
        .map(|entry| (entry.name, entry.value))
        .collect()
}

/// [`held_in`] against [`REGION`].
async fn held(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> BTreeMap<String, Option<String>> {
    held_in(router, REGION, case, group).await
}

/// What `group` reports for every parameter type at once, by name: the value and the
/// `Source` it carries.
async fn every_type(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
) -> BTreeMap<String, (Option<String>, Option<String>)> {
    let body = expect_ok(case, describe(router, group, &[]).await);
    reported(&body)
        .into_iter()
        .map(|entry| (entry.name, (entry.value, entry.source)))
        .collect()
}

/// The settings `remaining` names, as the `user` report carries them.
fn expected_held(remaining: &[(&str, &str)]) -> BTreeMap<String, Option<String>> {
    remaining
        .iter()
        .map(|(name, value)| ((*name).to_string(), Some((*value).to_string())))
        .collect()
}

// ---------------------------------------------------------------------------
// The rule: a reset drops the settings its scope names and keeps the rest
// ---------------------------------------------------------------------------

/// One row of the reset table.
struct Row {
    case: &'static str,
    /// The `DBParameterGroupFamily` the group is created with.
    family: &'static str,
    /// What the group is modified with before the reset.
    settings: &'static [Setting],
    /// The `Parameters` names the reset carries.
    names: &'static [&'static str],
    /// The `ResetAllParameters` value where the request carries one.
    reset_all: Option<bool>,
    /// The settings expected to remain, as `(name, value)`.
    remaining: &'static [(&'static str, &'static str)],
}

#[tokio::test]
async fn a_reset_drops_the_settings_its_scope_names_and_keeps_the_rest() {
    let router = emulator();
    let catalog =
        published_db_parameters(PUBLISHED_FAMILY).expect("this family publishes a catalog");
    assert!(
        published_db_parameters(UNPUBLISHED_FAMILY).is_none(),
        "case {:?}: the no-catalog rows need a family that publishes none, got {:?}",
        "the table's premise",
        published_db_parameters(UNPUBLISHED_FAMILY)
    );

    let rows = [
        Row {
            case: "the whole group, ResetAllParameters true",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &[],
            reset_all: Some(true),
            remaining: &[],
        },
        Row {
            // "By default, all parameters in the DB parameter group are reset to default
            // values": a request naming neither member names the whole group.
            case: "the whole group, neither member named",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &[],
            reset_all: None,
            remaining: &[],
        },
        Row {
            case: "one name, no flag",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &["autocommit"],
            reset_all: None,
            remaining: &[("wait_timeout", "600")],
        },
        Row {
            case: "one name beside an explicit ResetAllParameters false",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &["wait_timeout"],
            reset_all: Some(false),
            remaining: &[("autocommit", "0")],
        },
        Row {
            case: "two names of three",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("max_connections", "500", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &["autocommit", "max_connections"],
            reset_all: None,
            remaining: &[("wait_timeout", "600")],
        },
        Row {
            case: "every held name",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &["autocommit", "wait_timeout"],
            reset_all: None,
            remaining: &[],
        },
        Row {
            // `lower_case_table_names` is static in the published catalog, so "changes to
            // static parameters require a reboot" whatever the caller asked for: the change
            // is still waiting, and the reset is served all the same.
            case: "a static parameter whose change is still pending",
            family: PUBLISHED_FAMILY,
            settings: &[("lower_case_table_names", "1", None)],
            names: &["lower_case_table_names"],
            reset_all: None,
            remaining: &[],
        },
        Row {
            case: "a change the caller deferred to the next reboot",
            family: PUBLISHED_FAMILY,
            settings: &[("wait_timeout", "600", Some(APPLY_METHOD_PENDING_REBOOT))],
            names: &[],
            reset_all: Some(true),
            remaining: &[],
        },
        Row {
            case: "a name the group holds no setting under",
            family: PUBLISHED_FAMILY,
            settings: &[("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE))],
            names: &["general_log"],
            reset_all: None,
            remaining: &[("autocommit", "0")],
        },
        Row {
            // The whole-group scope turned OFF with no name beside it asks for no parameter
            // to be reset: the group is resolved and nothing of it changes.
            case: "ResetAllParameters false naming no parameter",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &[],
            reset_all: Some(false),
            remaining: &[("autocommit", "0"), ("wait_timeout", "600")],
        },
        Row {
            // The published catalog is a SAMPLE of the family's parameter list, so a name
            // outside it is a parameter the engine may well have: the group carries it, and
            // a reset naming it drops it.
            case: "a name the family publishes nothing for",
            family: PUBLISHED_FAMILY,
            settings: &[
                ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
                ("innodb_adaptive_hash_index", "0", Some(APPLY_METHOD_IMMEDIATE)),
            ],
            names: &["innodb_adaptive_hash_index"],
            reset_all: None,
            remaining: &[("autocommit", "0")],
        },
        Row {
            case: "a family that publishes no catalog, the whole group",
            family: UNPUBLISHED_FAMILY,
            settings: &[("dbm_database", "emu", Some(APPLY_METHOD_IMMEDIATE))],
            names: &[],
            reset_all: Some(true),
            remaining: &[],
        },
        Row {
            case: "a family that publishes no catalog, one name of two",
            family: UNPUBLISHED_FAMILY,
            settings: &[
                ("dbm_database", "emu", Some(APPLY_METHOD_IMMEDIATE)),
                ("sortheap", "256", Some(APPLY_METHOD_PENDING_REBOOT)),
            ],
            names: &["dbm_database"],
            reset_all: None,
            remaining: &[("sortheap", "256")],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let group = create_group(
            &router,
            case,
            &format!("reset-pg-{index}"),
            row.family,
        )
        .await;
        expect_ok(case, modify_in(&router, REGION, &group, row.settings).await);

        // The group holds exactly what the modify wrote, so a row that drops nothing cannot
        // pass on a group that was empty to begin with.
        let before = held(&router, case, &group).await;
        assert_eq!(
            before.len(),
            row.settings.len(),
            "case {case:?}: expected the group to hold the {} settings written, got {before:?}",
            row.settings.len()
        );

        let answered = expect_ok(case, reset(&router, &group, row.names, row.reset_all).await);
        assert_eq!(
            text(&answered, "DBParameterGroupName"),
            group,
            "case {case:?}: the answer must echo the group it reset, got {answered}"
        );

        // What the group holds of its own afterwards is the rule: the settings the scope did
        // not name, and nothing else.
        let after = held(&router, case, &group).await;
        assert_eq!(
            after,
            expected_held(row.remaining),
            "case {case:?}: expected the settings the reset did not name, got {after:?}"
        );

        // A dropped parameter the family publishes is reported at the family's own default
        // under `engine-default`, and one it publishes nothing for is reported not at all:
        // the group carries nothing for either.
        let reported_now = every_type(&router, case, &group).await;
        for (name, _, _) in row.settings {
            if row.remaining.iter().any(|(held, _)| held == name) {
                continue;
            }
            match catalog.iter().find(|spec| spec.name == *name) {
                Some(spec) if row.family == PUBLISHED_FAMILY => assert_eq!(
                    reported_now.get(*name),
                    Some(&(
                        Some(spec.default_value.to_string()),
                        Some(PARAMETER_SOURCE_ENGINE_DEFAULT.to_string())
                    )),
                    "case {case:?}: expected {name} back at its family's published default, \
                     got {reported_now:?}"
                ),
                _ => assert_eq!(
                    reported_now.get(*name),
                    None,
                    "case {case:?}: a dropped parameter the family publishes nothing for must \
                     be reported no longer, got {reported_now:?}"
                ),
            }
        }

        // The same reset again is the same answer and the same state: a reset asks for a
        // state, not for a change.
        let again = expect_ok(case, reset(&router, &group, row.names, row.reset_all).await);
        assert_eq!(
            text(&again, "DBParameterGroupName"),
            group,
            "case {case:?}: a repeated reset must answer the same group, got {again}"
        );
        assert_eq!(
            held(&router, case, &group).await,
            expected_held(row.remaining),
            "case {case:?}: a repeated reset must leave the same settings"
        );
    }
}

/// The group is resolved by the name as SENT, whatever its case: `DBParameterGroupName` "is
/// stored as a lowercase string", so a reset naming the group in another case resets that
/// same group and answers the stored, lowercased name.
#[tokio::test]
async fn a_reset_resolves_the_group_by_the_name_as_sent() {
    let router = emulator();
    let group = create_group(&router, "casing", "Reset-Case-PG", PUBLISHED_FAMILY).await;

    for (case, sent) in [
        ("the name as created", "Reset-Case-PG"),
        ("the name upper-cased", "RESET-CASE-PG"),
        ("the name lower-cased", "reset-case-pg"),
    ] {
        expect_ok(
            case,
            modify_in(
                &router,
                REGION,
                &group,
                &[("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE))],
            )
            .await,
        );
        let answered = expect_ok(case, reset(&router, sent, &["autocommit"], None).await);
        assert_eq!(
            text(&answered, "DBParameterGroupName"),
            group,
            "case {case:?}: expected the stored, lowercased name echoed, got {answered}"
        );
        let after = held(&router, case, &group).await;
        assert!(
            after.is_empty(),
            "case {case:?}: expected the setting dropped from the one group, got {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a reset refuses, and what it leaves behind when it does
// ---------------------------------------------------------------------------

/// One row of the refusal table: the request, the answer it must carry, and the sentence
/// fragment that answer must name, so a row cannot pass on a refusal of something else.
struct RefusalRow {
    case: &'static str,
    group: String,
    names: Vec<&'static str>,
    reset_all: Option<bool>,
    status: u16,
    code: &'static str,
    says: String,
}

#[tokio::test]
async fn a_reset_refuses_what_it_cannot_serve_and_changes_nothing() {
    let router = emulator();
    let group = create_group(&router, "refusals", "reset-guard-pg", PUBLISHED_FAMILY).await;
    let settings: &[Setting] = &[
        ("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE)),
        ("wait_timeout", "600", Some(APPLY_METHOD_IMMEDIATE)),
    ];
    expect_ok(
        "refusals",
        modify_in(&router, REGION, &group, settings).await,
    );

    // One name per position, past the documented maximum: the names need not be published
    // ones, since the bound is about how many a request carries.
    let over_bound: Vec<&'static str> = PADDING_NAMES
        .iter()
        .copied()
        .take(MAX_PARAMETERS_PER_MODIFY + 1)
        .collect();
    assert_eq!(
        over_bound.len(),
        MAX_PARAMETERS_PER_MODIFY + 1,
        "case {:?}: the bound row needs one name past the maximum of \
         {MAX_PARAMETERS_PER_MODIFY}, got {}",
        "the table's premise",
        over_bound.len()
    );

    let rows = [
        RefusalRow {
            case: "both scopes at once",
            group: group.clone(),
            names: vec!["autocommit"],
            reset_all: Some(true),
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            says: "ResetAllParameters".to_string(),
        },
        RefusalRow {
            case: "one name past the documented maximum",
            group: group.clone(),
            names: over_bound,
            reset_all: None,
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            says: MAX_PARAMETERS_PER_MODIFY.to_string(),
        },
        RefusalRow {
            case: "a group no region holds",
            group: "No-Such-Reset-PG".to_string(),
            names: vec![],
            reset_all: Some(true),
            status: 404,
            code: NOT_FOUND,
            // The fault names the group as the request spelled it.
            says: "DBParameterGroup not found: No-Such-Reset-PG".to_string(),
        },
        RefusalRow {
            case: "a parameter group Amazon RDS provides itself",
            group: format!("default.{PUBLISHED_FAMILY}"),
            names: vec![],
            reset_all: Some(true),
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            says: CANNOT_MODIFY_DEFAULT_PARAMETER_GROUP_MESSAGE.to_string(),
        },
    ];

    for row in &rows {
        let case = row.case;
        let body = expect_error(
            case,
            reset(&router, &row.group, &row.names, row.reset_all).await,
            row.status,
            row.code,
        );
        let message = text(&body, "Message");
        assert!(
            message.contains(&row.says),
            "case {case:?}: expected the refusal to name {:?}, got {message:?}",
            row.says
        );
    }

    // A `Parameters` entry carrying no `ParameterName` names no parameter to reset.
    let malformed = expect_error(
        "an entry carrying no ParameterName",
        call(
            &router,
            "ResetDBParameterGroup",
            &[
                ("DBParameterGroupName", group.as_str()),
                ("Parameters.Parameter.1.ParameterValue", "0"),
            ],
        )
        .await,
        400,
        INVALID_PARAMETER_VALUE,
    );
    assert!(
        text(&malformed, "Message").contains("ParameterName"),
        "case {:?}: expected the refusal to name the member at fault, got {malformed}",
        "an entry carrying no ParameterName"
    );

    // Not one of those refusals changed the group.
    let after = held(&router, "after the refusals", &group).await;
    assert_eq!(
        after,
        expected_held(&[("autocommit", "0"), ("wait_timeout", "600")]),
        "case {:?}: a refused reset must leave the group as it was, got {after:?}",
        "after the refusals"
    );

    // The maximum itself is SERVED, so the bound row above is not a blanket refusal: the
    // request carries the documented maximum, of which the two names the group holds are
    // dropped.
    let at_bound: Vec<&'static str> = std::iter::once("autocommit")
        .chain(
            PADDING_NAMES
                .iter()
                .copied()
                .take(MAX_PARAMETERS_PER_MODIFY - 1),
        )
        .collect();
    expect_ok(
        "the documented maximum",
        reset(&router, &group, &at_bound, None).await,
    );
    assert_eq!(
        held(&router, "the documented maximum", &group).await,
        expected_held(&[("wait_timeout", "600")]),
        "case {:?}: expected only the named setting dropped",
        "the documented maximum"
    );
}

/// Names for the rows that are about HOW MANY parameters a request carries rather than
/// about which: a name the published catalog does not carry is taken as sent, so these
/// occupy a position without naming a setting the group holds.
const PADDING_NAMES: [&str; 21] = [
    "emu_pad_01",
    "emu_pad_02",
    "emu_pad_03",
    "emu_pad_04",
    "emu_pad_05",
    "emu_pad_06",
    "emu_pad_07",
    "emu_pad_08",
    "emu_pad_09",
    "emu_pad_10",
    "emu_pad_11",
    "emu_pad_12",
    "emu_pad_13",
    "emu_pad_14",
    "emu_pad_15",
    "emu_pad_16",
    "emu_pad_17",
    "emu_pad_18",
    "emu_pad_19",
    "emu_pad_20",
    "emu_pad_21",
];

// ---------------------------------------------------------------------------
// Regional scope
// ---------------------------------------------------------------------------

/// RDS is regional: one group name holds a different group in each region, so a reset
/// signed for one region clears only that region's group, and a name only another region
/// holds is not found at all.
#[tokio::test]
async fn a_reset_clears_only_the_group_its_own_region_holds() {
    let router = emulator();
    let other = "eu-west-1";
    let settings: &[Setting] = &[("autocommit", "0", Some(APPLY_METHOD_IMMEDIATE))];

    for region in [REGION, other] {
        expect_ok(
            region,
            call_in(
                &router,
                region,
                "CreateDBParameterGroup",
                &[
                    ("DBParameterGroupName", "shared-reset-pg"),
                    ("DBParameterGroupFamily", PUBLISHED_FAMILY),
                    ("Description", "a group under test"),
                ],
            )
            .await,
        );
        expect_ok(
            region,
            modify_in(&router, region, "shared-reset-pg", settings).await,
        );
    }

    expect_ok(
        "reset in the first region",
        reset(&router, "shared-reset-pg", &[], Some(true)).await,
    );
    let here = held(&router, "the first region's group", "shared-reset-pg").await;
    assert!(
        here.is_empty(),
        "case {:?}: expected the reset region's group cleared, got {here:?}",
        "the first region's group"
    );
    let elsewhere = held_in(&router, other, "the other region's group", "shared-reset-pg").await;
    assert_eq!(
        elsewhere,
        expected_held(&[("autocommit", "0")]),
        "case {:?}: a reset in one region must not clear another's group",
        "the other region's group"
    );

    expect_ok(
        "a group only the first region holds",
        call(
            &router,
            "CreateDBParameterGroup",
            &[
                ("DBParameterGroupName", "only-here-reset-pg"),
                ("DBParameterGroupFamily", PUBLISHED_FAMILY),
                ("Description", "a group under test"),
            ],
        )
        .await,
    );
    expect_error(
        "reset a group only another region holds",
        reset_in(&router, other, "only-here-reset-pg", &[], Some(true)).await,
        404,
        NOT_FOUND,
    );
}
