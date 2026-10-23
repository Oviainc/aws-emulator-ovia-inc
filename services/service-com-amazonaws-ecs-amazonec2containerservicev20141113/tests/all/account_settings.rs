// SPDX-License-Identifier: CC0-1.0

//! The ACCOUNT SETTINGS surface, driven end to end over the shared wire harness in
//! [`crate::ecs_wire`]: `PutAccountSetting`/`PutAccountSettingDefault` write, and
//! `ListAccountSettings` and a cluster read answer from what was written.
//!
//! The rules under test:
//!
//! * `ListAccountSettings` with `effectiveSettings: true` reports, for EVERY setting
//!   name the model defines, the value in effect for the principal: that principal's own
//!   value when it has one, otherwise the account-wide default, otherwise the setting's
//!   documented default ("If `true`, the account settings for the root user or the
//!   default setting for the `principalArn` are returned").
//! * With `effectiveSettings: false` only the names that principal has WRITTEN are
//!   reported ("If `false`, the account settings for the `principalArn` are returned if
//!   they're set. Otherwise, no account settings are returned").
//! * `PutAccountSettingDefault` "modifies an account setting for all users on an account
//!   for whom no individual account setting has been specified", so it is the root
//!   user's setting and an individual `PutAccountSetting` shadows it for that principal
//!   alone.
//! * `Setting$type` is a property of the NAME: `aws_managed` for the one setting Amazon
//!   Web Services manages, `user` for the rest — and an `aws_managed` setting is
//!   read-only, so a write of it is refused.
//! * `ClusterSetting$value`: "If a cluster value is specified, it will override the
//!   `containerInsights` value set with PutAccountSetting or PutAccountSettingDefault" —
//!   so a cluster that set nothing reports the account setting in effect.
//! * `ListAccountSettings$maxResults`: "This value can be between 1 and 10. If this
//!   parameter isn't used, then ListAccountSettings returns up to 10 results and a
//!   nextToken value if applicable."
//! * `ListAccountSettings$value`: "You must also specify an account setting name to use
//!   this parameter."

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{list, ok, refused, service, string, ACCOUNT};

/// Every Amazon ECS account setting, from
/// `smithy_query(query="shape", shape="SettingName")` -> values, with the value it has
/// for an account that never wrote it and its `Setting$type`
/// (`smithy_query(query="shape", shape="Setting")` -> `type`; the `aws_managed` one is
/// "the `guardDutyActivate` account setting ... the only one Amazon Web Services
/// manages").
///
/// The defaults are the ones the model's own `PutAccountSetting$name`/`$value` docs
/// state: the ARN-and-id-format settings are on (the format every ARN this emulator
/// emits and the resource tagging it supports require it), `defaultLogDriverMode` is
/// `non-blocking` ("On June 25, 2025, Amazon ECS changed the default log driver mode
/// from `blocking` to `non-blocking`"), `fargateTaskRetirementWaitPeriod` is the 7
/// calendar days Fargate task maintenance waits, and every other opt-in feature is off
/// until a write turns it on.
const ACCOUNT_SETTINGS: [(&str, &str, &str); 11] = [
    ("awsvpcTrunking", "disabled", "user"),
    ("containerInsights", "disabled", "user"),
    ("containerInstanceLongArnFormat", "enabled", "user"),
    ("defaultLogDriverMode", "non-blocking", "user"),
    ("fargateEventWindows", "disabled", "user"),
    ("fargateFIPSMode", "disabled", "user"),
    ("fargateTaskRetirementWaitPeriod", "7", "user"),
    ("guardDutyActivate", "disabled", "aws_managed"),
    ("serviceLongArnFormat", "enabled", "user"),
    ("tagResourceAuthorization", "disabled", "user"),
    ("taskLongArnFormat", "enabled", "user"),
];

/// The largest page `ListAccountSettings$maxResults` allows, and the page size it uses
/// when a request gives none.
const MAX_PAGE: usize = 10;

/// The principal every unsigned-identity request is listed for: the one IAM user this
/// emulator authenticates every caller as.
fn authenticated_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// The root user — the principal whose settings ARE the account-wide defaults.
fn root_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:root")
}

/// One `ListAccountSettings` page.
async fn page(router: &MultiServiceRouter, request: Value, case: &str) -> Value {
    ok(router, "ListAccountSettings", request, case).await
}

/// Every setting `ListAccountSettings` reports for `request`, walking `nextToken` to the
/// end so a paginated answer is compared whole.
async fn all_settings(router: &MultiServiceRouter, request: Value, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    // One request per page, bounded by the catalog size so a token that never clears
    // fails the test instead of looping forever.
    for _ in 0..=ACCOUNT_SETTINGS.len() {
        let mut body = request.clone();
        if let Some(token) = &token {
            body["nextToken"] = json!(token);
        }
        let answer = page(router, body, case).await;
        collected.extend(list(&answer, "settings", case));
        token = answer["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return collected;
        }
    }
    panic!("case {case:?}: ListAccountSettings never stopped paging, got {collected:?}");
}

/// The `name` -> `value` pairs of a reported settings list.
fn values(settings: &[Value]) -> Vec<(String, String)> {
    settings
        .iter()
        .map(|setting| {
            (
                setting["name"].as_str().unwrap_or_default().to_string(),
                setting["value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The one setting `ListAccountSettings` reports for `name`, effective or not.
async fn effective(router: &MultiServiceRouter, name: &str, case: &str) -> Value {
    let settings = all_settings(
        router,
        json!({ "name": name, "effectiveSettings": true }),
        case,
    )
    .await;
    assert_eq!(
        settings.len(),
        1,
        "case {case:?}: one name has exactly one effective setting, got {settings:?}"
    );
    settings[0].clone()
}

#[tokio::test]
async fn effective_settings_report_every_modelled_name_with_its_documented_default() {
    let router = service();
    let case = "untouched account";

    let reported = all_settings(&router, json!({ "effectiveSettings": true }), case).await;

    let expected: Vec<(String, String)> = ACCOUNT_SETTINGS
        .iter()
        .map(|(name, value, _)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        values(&reported),
        expected,
        "case {case:?}: every account setting the model defines must be reported with \
         its documented default, got {reported:?}"
    );
    for (name, _, setting_type) in ACCOUNT_SETTINGS {
        let setting = reported
            .iter()
            .find(|setting| setting["name"] == json!(name))
            .unwrap_or_else(|| panic!("case {case:?}: {name} must be reported"));
        assert_eq!(
            setting["type"],
            json!(setting_type),
            "case {case:?}: {name} is managed by {setting_type}, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(root_principal()),
            "case {case:?}: a value nobody overrode comes from the account, so {name} \
             must name the root user, got {setting}"
        );
    }
}

#[tokio::test]
async fn a_read_of_the_account_settings_changes_nothing_a_later_read_sees() {
    let router = service();
    let case = "repeated read";

    let first = all_settings(&router, json!({ "effectiveSettings": true }), case).await;
    let second = all_settings(&router, json!({ "effectiveSettings": true }), case).await;
    assert_eq!(
        first, second,
        "case {case:?}: ListAccountSettings is a read, so two calls in a row must answer \
         the same thing, got {first:?} then {second:?}"
    );

    let unset = all_settings(&router, json!({ "effectiveSettings": false }), case).await;
    assert!(
        unset.is_empty(),
        "case {case:?}: no principal wrote a setting, so a non-effective listing must \
         report none, got {unset:?}"
    );
}

#[tokio::test]
async fn effective_settings_paginate_at_ten_results() {
    let router = service();

    // The rows differ along the page size: absent (the documented default of 10), each
    // published bound (1 and 10), and one inside the range.
    let cases: [(&str, Value, usize); 4] = [
        ("no maxResults", json!({}), MAX_PAGE),
        ("maxResults 1", json!({ "maxResults": 1 }), 1),
        ("maxResults 4", json!({ "maxResults": 4 }), 4),
        ("maxResults 10", json!({ "maxResults": 10 }), MAX_PAGE),
    ];

    for (case, extra, expected_page) in cases {
        let mut request = json!({ "effectiveSettings": true });
        for (member, value) in extra.as_object().expect("an object of extra members") {
            request[member] = value.clone();
        }
        let first = page(&router, request.clone(), case).await;
        let first_page = list(&first, "settings", case);
        assert_eq!(
            first_page.len(),
            expected_page,
            "case {case:?}: expected a page of {expected_page} settings, got {first_page:?}"
        );
        let token = string(&first, "nextToken", case);
        assert_eq!(
            token,
            ACCOUNT_SETTINGS[expected_page].0,
            "case {case:?}: the token must name the setting the next page starts at, got \
             {token:?}"
        );

        let walked = all_settings(&router, request, case).await;
        let expected: Vec<String> = ACCOUNT_SETTINGS
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        let names: Vec<String> = values(&walked).into_iter().map(|(name, _)| name).collect();
        assert_eq!(
            names, expected,
            "case {case:?}: walking every page must report each name exactly once, in \
             order, got {names:?}"
        );
    }
}

#[tokio::test]
async fn a_written_setting_is_what_a_later_list_reports() {
    let router = service();

    // One row per VALUE FORM the model documents: the enabled/disabled toggle, the
    // on/off spelling of the same toggle, the three-valued `containerInsights`, the
    // two-worded `defaultLogDriverMode`, and the numeric Fargate wait period.
    let cases: [(&str, &str, &str); 7] = [
        ("trunking on", "awsvpcTrunking", "enabled"),
        ("tag authorization on", "tagResourceAuthorization", "on"),
        ("tag authorization off", "tagResourceAuthorization", "off"),
        ("insights enhanced", "containerInsights", "enhanced"),
        ("log driver blocking", "defaultLogDriverMode", "blocking"),
        ("retire immediately", "fargateTaskRetirementWaitPeriod", "0"),
        ("retire in a fortnight", "fargateTaskRetirementWaitPeriod", "14"),
    ];

    for (case, name, value) in cases {
        let written = ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": value }),
            case,
        )
        .await;
        let setting = &written["setting"];
        assert_eq!(
            (setting["name"].clone(), setting["value"].clone()),
            (json!(name), json!(value)),
            "case {case:?}: the write must report the setting it stored, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(authenticated_principal()),
            "case {case:?}: a write that names no principal is the authenticated user's, \
             got {setting}"
        );

        // The round trip: the stored value is what both listings answer with.
        let explicit = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert_eq!(
            values(&explicit),
            vec![(name.to_string(), value.to_string())],
            "case {case:?}: a non-effective listing must report the setting this \
             principal wrote, got {explicit:?}"
        );
        let in_effect = effective(&router, name, case).await;
        assert_eq!(
            in_effect["value"],
            json!(value),
            "case {case:?}: the written value is the one in effect, got {in_effect}"
        );
        assert_eq!(
            in_effect["principalArn"],
            json!(authenticated_principal()),
            "case {case:?}: the value in effect comes from this principal, got {in_effect}"
        );

        // The filter the same listing offers: a value that is not the stored one matches
        // nothing, the stored one matches this setting.
        let filtered = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": true, "value": value }),
            case,
        )
        .await;
        assert_eq!(
            values(&filtered),
            vec![(name.to_string(), value.to_string())],
            "case {case:?}: filtering on the stored value must match it, got {filtered:?}"
        );
    }

    // Every write landed in the same store, so the non-effective listing reports them
    // all — one entry per NAME written, with the last value written for it.
    let all = all_settings(&router, json!({ "effectiveSettings": false }), "every write").await;
    assert_eq!(
        values(&all),
        vec![
            ("awsvpcTrunking".to_string(), "enabled".to_string()),
            ("containerInsights".to_string(), "enhanced".to_string()),
            ("defaultLogDriverMode".to_string(), "blocking".to_string()),
            (
                "fargateTaskRetirementWaitPeriod".to_string(),
                "14".to_string()
            ),
            ("tagResourceAuthorization".to_string(), "off".to_string()),
        ],
        "case \"every write\": a non-effective listing must report exactly the names \
         this principal wrote, got {all:?}"
    );
}

#[tokio::test]
async fn an_account_default_is_the_root_users_setting_and_a_principals_own_shadows_it() {
    let router = service();

    // The rows differ along the setting and the two values: what the account default is
    // set to, and what the one principal overrides it with.
    let cases: [(&str, &str, &str, &str); 3] = [
        ("insights", "containerInsights", "enabled", "enhanced"),
        ("trunking", "awsvpcTrunking", "enabled", "disabled"),
        (
            "retirement",
            "fargateTaskRetirementWaitPeriod",
            "14",
            "0",
        ),
    ];

    for (case, name, default_value, principal_value) in cases {
        let written = ok(
            &router,
            "PutAccountSettingDefault",
            json!({ "name": name, "value": default_value }),
            case,
        )
        .await;
        let setting = &written["setting"];
        assert_eq!(
            (setting["name"].clone(), setting["value"].clone()),
            (json!(name), json!(default_value)),
            "case {case:?}: the write must report the default it stored, got {setting}"
        );
        assert_eq!(
            setting["principalArn"],
            json!(root_principal()),
            "case {case:?}: an account-wide default is the root user's setting, got \
             {setting}"
        );

        // The default is what every principal with no setting of its own gets.
        let in_effect = effective(&router, name, case).await;
        assert_eq!(
            (
                in_effect["value"].clone(),
                in_effect["principalArn"].clone()
            ),
            (json!(default_value), json!(root_principal())),
            "case {case:?}: the account default is in effect for the authenticated user, \
             got {in_effect}"
        );
        // ... and it is the root user's own setting, so the root user has it explicitly.
        let root_explicit = all_settings(
            &router,
            json!({ "name": name, "principalArn": root_principal(), "effectiveSettings": false }),
            case,
        )
        .await;
        assert_eq!(
            values(&root_explicit),
            vec![(name.to_string(), default_value.to_string())],
            "case {case:?}: the root user holds the account default explicitly, got \
             {root_explicit:?}"
        );
        // The authenticated user wrote nothing, so a non-effective listing for it
        // reports nothing of this name.
        let own_explicit = all_settings(
            &router,
            json!({ "name": name, "effectiveSettings": false }),
            case,
        )
        .await;
        assert!(
            own_explicit.is_empty(),
            "case {case:?}: a principal that wrote nothing has no setting of its own, \
             got {own_explicit:?}"
        );

        // An individual write shadows the default for that principal alone.
        ok(
            &router,
            "PutAccountSetting",
            json!({ "name": name, "value": principal_value }),
            case,
        )
        .await;
        let shadowed = effective(&router, name, case).await;
        assert_eq!(
            (shadowed["value"].clone(), shadowed["principalArn"].clone()),
            (
                json!(principal_value),
                json!(authenticated_principal())
            ),
            "case {case:?}: a principal's own setting overrides the account default, got \
             {shadowed}"
        );
        let root_after = all_settings(
            &router,
            json!({ "name": name, "principalArn": root_principal(), "effectiveSettings": true }),
            case,
        )
        .await;
        assert_eq!(
            values(&root_after),
            vec![(name.to_string(), default_value.to_string())],
            "case {case:?}: one principal's write must leave the account default where it \
             was, got {root_after:?}"
        );
    }
}

#[tokio::test]
async fn a_cluster_that_sets_nothing_inherits_the_account_setting() {
    // The rows differ along where `containerInsights` comes from and what it is: the
    // untouched default, each value an account default can hold, and a cluster that
    // overrides the account with one of its own.
    let cases: [(&str, Option<&str>, Option<&str>, &str); 5] = [
        ("nothing set", None, None, "disabled"),
        ("account enabled", Some("enabled"), None, "enabled"),
        ("account enhanced", Some("enhanced"), None, "enhanced"),
        ("cluster overrides", Some("enabled"), Some("disabled"), "disabled"),
        ("cluster enhances", Some("disabled"), Some("enhanced"), "enhanced"),
    ];

    for (index, (case, account_value, cluster_value, expected)) in cases.iter().enumerate() {
        // A fresh engine per row, so one row's account setting cannot leak into another.
        let router = service();
        if let Some(value) = account_value {
            ok(
                &router,
                "PutAccountSettingDefault",
                json!({ "name": "containerInsights", "value": value }),
                case,
            )
            .await;
        }
        let cluster_name = format!("inherit-{index}");
        let mut request = json!({ "clusterName": cluster_name });
        if let Some(value) = cluster_value {
            request["settings"] = json!([{ "name": "containerInsights", "value": value }]);
        }
        let created = ok(&router, "CreateCluster", request, case).await;
        assert_eq!(
            list(&created["cluster"], "settings", case),
            vec![json!({ "name": "containerInsights", "value": expected })],
            "case {case:?}: a created cluster must report the setting in effect for it, \
             got {created}"
        );

        // The same answer on a later READ of the stored cluster.
        let described = ok(
            &router,
            "DescribeClusters",
            json!({ "clusters": [cluster_name], "include": ["SETTINGS"] }),
            case,
        )
        .await;
        let clusters = list(&described, "clusters", case);
        assert_eq!(
            list(&clusters[0], "settings", case),
            vec![json!({ "name": "containerInsights", "value": expected })],
            "case {case:?}: a described cluster must report the setting in effect for it, \
             got {described}"
        );
    }
}

#[tokio::test]
async fn an_account_setting_amazon_manages_is_read_only() {
    let router = service();

    // `guardDutyActivate` is the one `aws_managed` setting, and both writes refuse it.
    for operation in ["PutAccountSetting", "PutAccountSettingDefault"] {
        let case = format!("{operation} on guardDutyActivate");
        refused(
            &router,
            operation,
            json!({ "name": "guardDutyActivate", "value": "enabled" }),
            "InvalidParameterException",
            &case,
        )
        .await;
    }

    // The refusal stored nothing: the setting is still the value Amazon Web Services
    // reports for it.
    let setting = effective(&router, "guardDutyActivate", "after the refusals").await;
    assert_eq!(
        (setting["value"].clone(), setting["type"].clone()),
        (json!("disabled"), json!("aws_managed")),
        "case \"after the refusals\": a read-only setting must keep its value, got \
         {setting}"
    );
}

#[tokio::test]
async fn a_request_outside_the_documented_parameters_is_refused() {
    let router = service();

    // The rows span the documented constraints: the values a NAME accepts (a value from
    // another setting's domain, a wait period outside 0/7/14), the principal a setting
    // may belong to, the `maxResults` range (one past each published bound) and the
    // `value` filter's dependency on `name`.
    let cases: [(&str, &str, Value); 10] = [
        (
            "a log driver mode is not an insights value",
            "PutAccountSetting",
            json!({ "name": "containerInsights", "value": "non-blocking" }),
        ),
        (
            "an insights value is not a log driver mode",
            "PutAccountSettingDefault",
            json!({ "name": "defaultLogDriverMode", "value": "enabled" }),
        ),
        (
            "an undocumented wait period",
            "PutAccountSettingDefault",
            json!({ "name": "fargateTaskRetirementWaitPeriod", "value": "3" }),
        ),
        (
            "a principal that is not an ARN",
            "PutAccountSetting",
            json!({ "name": "awsvpcTrunking", "value": "enabled", "principalArn": "emulator" }),
        ),
        (
            "listing for a principal that is not an ARN",
            "ListAccountSettings",
            json!({ "principalArn": "emulator", "effectiveSettings": true }),
        ),
        (
            "maxResults below the range",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "maxResults": -1 }),
        ),
        (
            "maxResults above the range",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "maxResults": 11 }),
        ),
        (
            "a value filter with no name",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "value": "disabled" }),
        ),
        (
            "a token this operation never minted",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "nextToken": "zzz" }),
        ),
        (
            "a token that is an empty string",
            "ListAccountSettings",
            json!({ "effectiveSettings": true, "nextToken": "" }),
        ),
    ];

    for (case, operation, request) in cases {
        refused(
            &router,
            operation,
            request,
            "InvalidParameterException",
            case,
        )
        .await;
    }

    // None of the refusals wrote anything: every setting is still its default.
    let reported = all_settings(
        &router,
        json!({ "effectiveSettings": true }),
        "after the refusals",
    )
    .await;
    let expected: Vec<(String, String)> = ACCOUNT_SETTINGS
        .iter()
        .map(|(name, value, _)| ((*name).to_string(), (*value).to_string()))
        .collect();
    assert_eq!(
        values(&reported),
        expected,
        "case \"after the refusals\": a refused write must store nothing, got {reported:?}"
    );
}

#[tokio::test]
async fn a_principal_an_account_setting_may_belong_to_is_an_iam_arn() {
    // `PutAccountSetting$principalArn`: "The ARN of the principal, which can be a user,
    // role, or the root user" — an IAM ARN, `arn:<partition>:iam::<account>:<resource>`.
    // The rows differ along the FORM of the value: the principal kinds AWS mints, a
    // partition other than `aws`, a resource with a path — and, refused, each field that
    // makes it something else, plus a key longer than the emulator stores.
    let accepted: [(&str, String); 4] = [
        ("the root user", root_principal()),
        ("a user", authenticated_principal()),
        ("a role", format!("arn:aws:iam::{ACCOUNT}:role/deployer")),
        (
            "a user under a path",
            format!("arn:aws-us-gov:iam::{ACCOUNT}:user/team/lead"),
        ),
    ];
    let refused_arns: [(&str, String); 6] = [
        ("not an ARN at all", "emulator".to_string()),
        ("empty fields", "arn::iam:".to_string()),
        ("no resource", format!("arn:aws:iam::{ACCOUNT}:")),
        (
            "a region on a global service",
            format!("arn:aws:iam:us-west-2:{ACCOUNT}:user/emulator"),
        ),
        (
            "another service entirely",
            format!("arn:aws:ecs::{ACCOUNT}:cluster/default"),
        ),
        (
            "a key longer than the store holds",
            format!("arn:aws:iam::{ACCOUNT}:user/{}", "p".repeat(2048)),
        ),
    ];

    for (case, principal_arn) in accepted {
        let router = service();
        let written = ok(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal_arn,
            }),
            case,
        )
        .await;
        assert_eq!(
            written["setting"]["principalArn"],
            json!(principal_arn),
            "case {case:?}: the write must report the principal it stored the setting \
             for, got {written}"
        );
        let stored = all_settings(
            &router,
            json!({
                "name": "awsvpcTrunking",
                "principalArn": principal_arn,
                "effectiveSettings": false,
            }),
            case,
        )
        .await;
        assert_eq!(
            values(&stored),
            vec![("awsvpcTrunking".to_string(), "enabled".to_string())],
            "case {case:?}: a later listing for that principal must report the stored \
             setting, got {stored:?}"
        );
    }

    let router = service();
    for (case, principal_arn) in refused_arns {
        refused(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal_arn,
            }),
            "InvalidParameterException",
            case,
        )
        .await;
        refused(
            &router,
            "ListAccountSettings",
            json!({ "principalArn": principal_arn, "effectiveSettings": true }),
            "InvalidParameterException",
            case,
        )
        .await;
    }
}

#[tokio::test]
async fn the_store_holds_settings_for_a_bounded_number_of_principals() {
    let router = service();
    // The emulator-chosen cap on distinct principals
    // (`state::MAX_ACCOUNT_SETTING_PRINCIPALS`), so a caller cannot grow the in-memory
    // store without limit.
    const MAX_PRINCIPALS: usize = 64;

    let principal = |index: usize| format!("arn:aws:iam::{ACCOUNT}:user/principal-{index}");

    for index in 0..MAX_PRINCIPALS {
        let case = format!("principal {index}");
        ok(
            &router,
            "PutAccountSetting",
            json!({
                "name": "awsvpcTrunking",
                "value": "enabled",
                "principalArn": principal(index),
            }),
            &case,
        )
        .await;
    }

    // A further NAME for a principal already stored is still accepted: the cap counts
    // principals, not settings.
    ok(
        &router,
        "PutAccountSetting",
        json!({
            "name": "containerInsights",
            "value": "enabled",
            "principalArn": principal(0),
        }),
        "a second setting for a stored principal",
    )
    .await;

    let case = "one principal too many";
    refused(
        &router,
        "PutAccountSetting",
        json!({
            "name": "awsvpcTrunking",
            "value": "enabled",
            "principalArn": principal(MAX_PRINCIPALS),
        }),
        "ClientException",
        case,
    )
    .await;

    // The refusal stored nothing for that principal, and left every stored one alone.
    let refused_principal = all_settings(
        &router,
        json!({
            "principalArn": principal(MAX_PRINCIPALS),
            "effectiveSettings": false,
        }),
        case,
    )
    .await;
    assert!(
        refused_principal.is_empty(),
        "case {case:?}: a refused write must store nothing, got {refused_principal:?}"
    );
    let first = all_settings(
        &router,
        json!({ "principalArn": principal(0), "effectiveSettings": false }),
        case,
    )
    .await;
    assert_eq!(
        values(&first),
        vec![
            ("awsvpcTrunking".to_string(), "enabled".to_string()),
            ("containerInsights".to_string(), "enabled".to_string()),
        ],
        "case {case:?}: the settings already stored must survive the refusal, got \
         {first:?}"
    );
}

/// The catalog the handlers resolve values over must cover EXACTLY the `SettingName`
/// enum: a name the model gains and the catalog misses would be reported with a value
/// nobody stored, and a name the catalog holds and the model drops would be reported for
/// a setting the service does not have. Pinned against the model's own generated value
/// list, so a model update fails here rather than silently.
#[test]
fn the_catalog_covers_exactly_the_modelled_setting_names() {
    let case = "SettingName";
    let mut modelled: Vec<&str> =
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::ops::types::SETTING_NAME_VALUES
            .to_vec();
    modelled.sort_unstable();
    let mut catalogued: Vec<&str> =
        service_com_amazonaws_ecs_amazonec2containerservicev20141113::state::ACCOUNT_SETTINGS
            .iter()
            .map(|setting| setting.name)
            .collect();
    catalogued.sort_unstable();
    assert_eq!(
        catalogued, modelled,
        "case {case:?}: the account-settings catalog must cover exactly the modelled \
         setting names, got {catalogued:?} against {modelled:?}"
    );

    // ... and the table this suite asserts over is the same set, so the wire
    // expectations above cannot drift from the catalog either.
    let mut tabled: Vec<&str> = ACCOUNT_SETTINGS.iter().map(|(name, _, _)| *name).collect();
    tabled.sort_unstable();
    assert_eq!(
        tabled, modelled,
        "case {case:?}: this suite's table must cover exactly the modelled setting \
         names, got {tabled:?} against {modelled:?}"
    );
}
