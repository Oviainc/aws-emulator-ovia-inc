// SPDX-License-Identifier: CC0-1.0

//! The response members WAFv2 populates on EVERY successful save and read.
//!
//! A WAFv2 response carries the members the service derives or defaults
//! server-side, not only the ones the request happened to supply: an entity saved
//! without a description reports `Description` as `""`, a web ACL with no rules
//! reports `Rules` as `[]`, `OnSourceDDoSProtectionConfig` carries the account
//! default, a rate-based statement carries the documented evaluation window, and
//! every non-empty `ListWebACLs` page carries a `NextMarker`. Each suite below
//! asserts that over a TABLE of saves rather than over one request, and reads every
//! save back through `GetWebACL` / `GetIPSet` / `ListWebACLs`.
//!
//! The harness is the one in [`crate::web_acl_lifecycle`]: every call goes through
//! the router's front door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{rule, sqli_statement, visibility, Waf};

/// The `LowReputationMode` the model marks as the account default:
/// `OnSourceDDoSProtectionConfig$ALBLowReputationMode` documents
/// `ACTIVE_UNDER_DDOS` as "enabled by default whenever a web ACL is associated with
/// an Application Load Balancer".
const DEFAULT_DDOS_MODE: &str = "ACTIVE_UNDER_DDOS";

/// The other value `LowReputationMode` declares
/// (`smithy_query(query="shape",shape="LowReputationMode")` →
/// `ACTIVE_UNDER_DDOS`, `ALWAYS_ON`) — the entry the issue did not cite.
const NON_DEFAULT_DDOS_MODE: &str = "ALWAYS_ON";

/// `count` distinct blocking SQLi rules, each with its own name and priority, as a
/// JSON `Rules` list.
fn sqli_rules(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| {
                let mut statement = rule(&format!("rule-{index}"), sqli_statement());
                statement["Priority"] = json!(index);
                statement
            })
            .collect(),
    )
}

/// A blocking rule whose statement is a rate-based statement, carrying
/// `EvaluationWindowSec` only when `window` is `Some`.
fn rate_based_rule(name: &str, window: Option<i64>) -> Value {
    let mut statement = json!({
        "RateBasedStatement": {
            "Limit": 1000,
            "AggregateKeyType": "IP",
        }
    });
    if let Some(window) = window {
        statement["RateBasedStatement"]["EvaluationWindowSec"] = json!(window);
    }
    rule(name, statement)
}

/// A `CreateWebACL` body in REGIONAL scope. `Description` and
/// `OnSourceDDoSProtectionConfig` are present only when the case supplies them, so
/// an absent member is really absent on the wire.
fn create_body(
    name: &str,
    description: Option<&str>,
    rules: &Value,
    ddos_mode: Option<&str>,
) -> Value {
    let mut body = json!({
        "Name": name,
        "Scope": "REGIONAL",
        "DefaultAction": { "Allow": {} },
        "VisibilityConfig": visibility(name),
        "Rules": rules,
    });
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(ddos_mode) = ddos_mode {
        body["OnSourceDDoSProtectionConfig"] = json!({ "ALBLowReputationMode": ddos_mode });
    }
    body
}

/// `GetWebACL` for a web ACL in REGIONAL scope.
async fn get_web_acl(waf: &Waf, name: &str, id: &str) -> Value {
    waf.ok(
        "GetWebACL",
        json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
    )
    .await
}

/// The summary `ListWebACLs` answers for `name`, in REGIONAL scope.
async fn listed_summary(waf: &Waf, name: &str, case: &str) -> Value {
    let listed = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
    listed["WebACLs"]
        .as_array()
        .expect("WebACLs")
        .iter()
        .find(|summary| summary["Name"] == json!(name))
        .unwrap_or_else(|| panic!("case {case:?}: ListWebACLs must list {name:?}, got {listed}"))
        .clone()
}

/// One row of [`every_web_acl_response_carries_description_rules_and_the_ddos_mode`]:
/// the case name, the `Description` the save supplies (if any), its `Rules`, the
/// `ALBLowReputationMode` it supplies (if any) and the mode the read must answer.
type WebAclCase<'a> = (
    &'a str,
    Option<&'a str>,
    Value,
    Option<&'a str>,
    &'a str,
);

/// Every web ACL response carries `Description`, `Rules` and
/// `OnSourceDDoSProtectionConfig`, whether or not the save supplied them: the
/// description of a web ACL saved without one is the empty string, a rule-less web
/// ACL's `Rules` is the empty list, and the DDoS-protection configuration is the
/// account default unless the save named a mode.
///
/// One row per way a save can differ along what the rule generalises over: the
/// description absent, present, and at the modelled maximum length
/// (`EntityDescription`, length 1..256 — its pattern
/// `^[\w+=:#@/\-,\.][\w+=:#@/\-,\.\s]+[\w+=:#@/\-,\.]$` makes three characters the
/// shortest legal one, so there is no one-character row); the rule count 0, 1 and
/// 3; and the DDoS mode absent, the default named explicitly, and the non-default.
#[tokio::test]
async fn every_web_acl_response_carries_description_rules_and_the_ddos_mode() {
    // 256 characters: `EntityDescription`'s published maximum length.
    let longest_description = "d".repeat(256);
    let cases: Vec<WebAclCase<'_>> = vec![
        (
            "no description, no rules, no DDoS config",
            None,
            sqli_rules(0),
            None,
            DEFAULT_DDOS_MODE,
        ),
        (
            "no description, one rule",
            None,
            sqli_rules(1),
            None,
            DEFAULT_DDOS_MODE,
        ),
        (
            "a description with a space and punctuation, three rules",
            Some("three rules, one description"),
            sqli_rules(3),
            None,
            DEFAULT_DDOS_MODE,
        ),
        (
            "a description at the modelled maximum length",
            Some(longest_description.as_str()),
            sqli_rules(1),
            None,
            DEFAULT_DDOS_MODE,
        ),
        (
            "the non-default DDoS mode, named explicitly",
            None,
            sqli_rules(0),
            Some(NON_DEFAULT_DDOS_MODE),
            NON_DEFAULT_DDOS_MODE,
        ),
        (
            "the default DDoS mode, named explicitly",
            Some("explicit.default"),
            sqli_rules(1),
            Some(DEFAULT_DDOS_MODE),
            DEFAULT_DDOS_MODE,
        ),
    ];

    let waf = Waf::new();
    for (index, (case, description, rules, requested_mode, expected_mode)) in
        cases.into_iter().enumerate()
    {
        let name = format!("acl-{index}");
        let expected_description = description.unwrap_or("");
        let rule_count = rules.as_array().expect("a rule list").len();

        let created = waf
            .ok(
                "CreateWebACL",
                create_body(&name, description, &rules, requested_mode),
            )
            .await;
        assert_eq!(
            created["Summary"]["Description"],
            json!(expected_description),
            "case {case:?}: CreateWebACL's Summary must carry Description \
             {expected_description:?}, got {created}"
        );
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();

        let got = get_web_acl(&waf, &name, &id).await;
        let web_acl = &got["WebACL"];
        assert_eq!(
            web_acl["Description"],
            json!(expected_description),
            "case {case:?}: GetWebACL must answer Description {expected_description:?}, \
             got {got}"
        );
        assert_eq!(
            web_acl["Rules"].as_array().map(Vec::len),
            Some(rule_count),
            "case {case:?}: GetWebACL must answer Rules as a list of {rule_count}, got {got}"
        );
        assert_eq!(
            web_acl["OnSourceDDoSProtectionConfig"],
            json!({ "ALBLowReputationMode": expected_mode }),
            "case {case:?}: GetWebACL must answer the {expected_mode} DDoS mode, got {got}"
        );

        let summary = listed_summary(&waf, &name, case).await;
        assert_eq!(
            summary["Description"],
            json!(expected_description),
            "case {case:?}: the ListWebACLs summary must carry Description \
             {expected_description:?}, got {summary}"
        );
    }
}

/// An `IPSet` response carries `Description` the same way a web ACL's does: the
/// empty string when the save supplied none, both in the summary `CreateIPSet`
/// answers and in the set `GetIPSet` answers. One row per description form and per
/// address family.
#[tokio::test]
async fn every_ip_set_response_carries_a_description() {
    let cases: &[(&str, Option<&str>, &str, &str)] = &[
        ("no description, IPv4", None, "IPV4", "10.0.0.0/8"),
        (
            "a description, IPv4",
            Some("has.a.description"),
            "IPV4",
            "192.0.2.0/24",
        ),
        ("no description, IPv6", None, "IPV6", "2001:db8::/32"),
        (
            "a description with a space, IPv6",
            Some("ipv6 blocks, described"),
            "IPV6",
            "2001:db8:1::/48",
        ),
    ];

    let waf = Waf::new();
    for (index, (case, description, version, address)) in cases.iter().enumerate() {
        let name = format!("ips-{index}");
        let expected_description = description.unwrap_or("");
        let mut body = json!({
            "Name": name,
            "Scope": "REGIONAL",
            "IPAddressVersion": version,
            "Addresses": [address],
        });
        if let Some(description) = description {
            body["Description"] = json!(description);
        }

        let created = waf.ok("CreateIPSet", body).await;
        assert_eq!(
            created["Summary"]["Description"],
            json!(expected_description),
            "case {case:?}: CreateIPSet's Summary must carry Description \
             {expected_description:?}, got {created}"
        );
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();

        let got = waf
            .ok(
                "GetIPSet",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
            )
            .await;
        assert_eq!(
            got["IPSet"]["Description"],
            json!(expected_description),
            "case {case:?}: GetIPSet must answer Description {expected_description:?}, \
             got {got}"
        );
        assert_eq!(
            got["IPSet"]["Addresses"],
            json!([address]),
            "case {case:?}: GetIPSet must answer the stored addresses, got {got}"
        );
    }
}

/// A rate-based statement reads back with the evaluation window WAF stores for it.
/// `RateBasedStatement$EvaluationWindowSec`: "Valid settings are 60, 120, 300, and
/// 600 … Default: 300 (5 minutes)" — so a save that names one of those settings
/// reads it back, and a save that names none reads back the documented default
/// rather than the member's modelled zero value.
#[tokio::test]
async fn a_rate_based_statement_reads_back_with_the_documented_evaluation_window() {
    // Every setting the member's doc publishes, plus the absent case.
    let cases: &[(&str, Option<i64>, i64)] = &[
        ("absent", None, 300),
        ("60", Some(60), 60),
        ("120", Some(120), 120),
        ("300", Some(300), 300),
        ("600", Some(600), 600),
    ];

    let waf = Waf::new();
    for (index, (case, requested, expected)) in cases.iter().enumerate() {
        let name = format!("rate-acl-{index}");
        let rules = json!([rate_based_rule("rate-limit", *requested)]);
        let created = waf
            .ok("CreateWebACL", create_body(&name, None, &rules, None))
            .await;
        let id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();

        let got = get_web_acl(&waf, &name, &id).await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"]["RateBasedStatement"]["EvaluationWindowSec"],
            json!(expected),
            "case {case:?}: GetWebACL must answer EvaluationWindowSec {expected}, got {got}"
        );
        // The rest of the statement is stored verbatim beside the default.
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"]["RateBasedStatement"]["Limit"],
            json!(1000),
            "case {case:?}: GetWebACL must answer the stored rate limit, got {got}"
        );
    }
}

/// An update reports the same server-side members a create does: a web ACL updated
/// without a description, without rules and without a DDoS configuration reads back
/// with `""`, `[]` and the account default — and a rate-based rule saved by an
/// update gets the documented evaluation window too. An update is a whole-resource
/// overwrite, so this is the case where a member that WAS set becomes unset.
#[tokio::test]
async fn an_update_reports_the_same_defaults_as_a_create() {
    let waf = Waf::new();
    let created = waf
        .ok(
            "CreateWebACL",
            create_body(
                "updated-acl",
                Some("first generation"),
                &json!([rate_based_rule("rate-limit", Some(600))]),
                Some(NON_DEFAULT_DDOS_MODE),
            ),
        )
        .await;
    let summary = &created["Summary"];
    let id = summary["Id"].as_str().expect("Summary.Id").to_string();
    let lock_token = summary["LockToken"]
        .as_str()
        .expect("Summary.LockToken")
        .to_string();

    let update_body = |lock: &str, rules: Value| {
        json!({
            "Name": "updated-acl",
            "Scope": "REGIONAL",
            "Id": id,
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility("updated-acl"),
            "LockToken": lock,
            "Rules": rules,
        })
    };

    // Everything the create set is omitted: the update clears it to the default.
    let updated = waf
        .ok("UpdateWebACL", update_body(&lock_token, json!([])))
        .await;
    let next_lock = updated["NextLockToken"]
        .as_str()
        .expect("NextLockToken")
        .to_string();
    let got = get_web_acl(&waf, "updated-acl", &id).await;
    assert_eq!(
        got["WebACL"]["Description"],
        json!(""),
        "an update that omits Description must read back as the empty string, got {got}"
    );
    assert_eq!(
        got["WebACL"]["Rules"],
        json!([]),
        "an update that omits Rules must read back as the empty list, got {got}"
    );
    assert_eq!(
        got["WebACL"]["OnSourceDDoSProtectionConfig"],
        json!({ "ALBLowReputationMode": DEFAULT_DDOS_MODE }),
        "an update that omits OnSourceDDoSProtectionConfig must read back as the \
         account default, got {got}"
    );
    assert_eq!(
        got["WebACL"]["Capacity"],
        json!(0),
        "a web ACL with no rules consumes no WCUs, got {got}"
    );
    let summary = listed_summary(&waf, "updated-acl", "after the clearing update").await;
    assert_eq!(
        summary["Description"],
        json!(""),
        "the ListWebACLs summary must carry the cleared description, got {summary}"
    );

    // A rate-based rule saved by an update gets the documented window as well.
    waf.ok(
        "UpdateWebACL",
        update_body(&next_lock, json!([rate_based_rule("rate-limit", None)])),
    )
    .await;
    let got = get_web_acl(&waf, "updated-acl", &id).await;
    assert_eq!(
        got["WebACL"]["Rules"][0]["Statement"]["RateBasedStatement"]["EvaluationWindowSec"],
        json!(300),
        "an update that omits EvaluationWindowSec must read back as the documented \
         default, got {got}"
    );
}

/// `ListWebACLs` answers every non-empty page with a `NextMarker`: the name of the
/// last summary on the page, whether or not the page is full. An empty page has no
/// last name and `ListWebACLsResponse$NextMarker` is length 1..256, so it carries
/// none — which is what ends a paginator's loop. One row per collection size,
/// including the empty collection.
#[tokio::test]
async fn list_web_acls_reports_the_last_summary_name_as_next_marker() {
    for count in [0_usize, 1, 2, 5] {
        let case = format!("{count} web ACLs");
        let waf = Waf::new();
        let names: Vec<String> = (0..count).map(|index| format!("marker-acl-{index}")).collect();
        for name in &names {
            waf.ok("CreateWebACL", create_body(name, None, &sqli_rules(0), None))
                .await;
        }

        let page = waf.ok("ListWebACLs", json!({ "Scope": "REGIONAL" })).await;
        assert_eq!(
            page["WebACLs"].as_array().map(Vec::len),
            Some(count),
            "case {case:?}: the page must hold every web ACL, got {page}"
        );
        assert_eq!(
            page["NextMarker"].as_str(),
            names.last().map(String::as_str),
            "case {case:?}: NextMarker must be the last summary's name, got {page}"
        );
    }
}
