// SPDX-License-Identifier: CC0-1.0

//! The CHALLENGE immunity floor, end to end over the wire.
//!
//! The rule these suites assert is the one
//! `ImmunityTimeProperty$ImmunityTime` documents: "The amount of time, in seconds,
//! that a `CAPTCHA` or challenge timestamp is considered valid by WAF. The default
//! setting is 300. **For the Challenge action, the minimum setting is 300.**" So a
//! save carrying a CHALLENGE immunity below 300 seconds is refused with 400
//! `WAFInvalidParameterException` — the one exception of this service whose body
//! describes the call, so it names the challenge configuration as its `Field`, the
//! rejected value as its `Parameter` and the rule as its `Reason` — and nothing is
//! written: no entity is created, and an update rotates no lock token. The modelled
//! `@range(60..259200)` the member carries is the CAPTCHA floor and the shared
//! ceiling, and is enforced at the request boundary.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in the VALUE (each published bound of the modelled range, the
//! Challenge floor, one either side of it, and one past each modelled bound), in
//! WHERE the challenge configuration sits (on the entity, on a rule), in which
//! entity carries it (a web ACL, a rule group) and in which operation saves it
//! (create, update). The harness is the one in [`crate::web_acl_lifecycle`]: every
//! call goes through the router's front door, so what these tests observe is what an
//! SDK observes.

use serde_json::{json, Value};

use crate::web_acl_lifecycle::{create_web_acl, rule, sqli_statement, visibility, Waf};

/// The Challenge floor itself, as the member's documentation states it ("For the
/// Challenge action, the minimum setting is 300"). Spelled out here independently of
/// the service's own constant, so this suite pins the bound and not merely its reuse.
const CHALLENGE_FLOOR: i64 = 300;

/// The `ParameterExceptionField` value that names a challenge configuration
/// (`smithy_query(query="shape",shape="ParameterExceptionField")` lists
/// `CHALLENGE_CONFIG` among its values).
const FIELD_CHALLENGE_CONFIG: &str = "CHALLENGE_CONFIG";

/// A declared capacity above what any rule list below costs.
const AMPLE_CAPACITY: i64 = 500;

/// The `message` member of an awsJson error body, whichever casing the shape models.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The awsJson error code of a refusal, from the body's `__type`.
fn error_code(body: &Value) -> String {
    body["__type"].as_str().unwrap_or_default().to_string()
}

/// One place a challenge configuration can sit: the case name and a builder
/// answering `(the entity-level ChallengeConfig, the Rules)` for an immunity value.
type Placement = (&'static str, fn(i64) -> (Value, Value));

/// A `ChallengeConfig` (or `CaptchaConfig`) carrying `immunity_time`.
fn immunity(immunity_time: i64) -> Value {
    json!({ "ImmunityTimeProperty": { "ImmunityTime": immunity_time } })
}

/// A rule whose own `ChallengeConfig` carries `immunity_time`.
fn rule_with_challenge(name: &str, immunity_time: i64) -> Value {
    let mut built = rule(name, sqli_statement());
    built["ChallengeConfig"] = immunity(immunity_time);
    built
}

/// One immunity value, and what WAF does with it as a CHALLENGE immunity.
struct ImmunityCase {
    case: &'static str,
    immunity_time: i64,
    /// `None` when the save is accepted; otherwise the error code the refusal
    /// carries — the service's own `WAFInvalidParameterException` for a value the
    /// Challenge floor refuses, and the request validator's `ValidationException`
    /// for one outside the member's modelled range.
    refusal: Option<&'static str>,
}

/// Every row of the value table: each published bound of the modelled range, one
/// past each, the Challenge floor and one either side of it.
fn immunity_cases() -> Vec<ImmunityCase> {
    vec![
        ImmunityCase {
            case: "one below the modelled range floor",
            immunity_time: 59,
            refusal: Some("ValidationException"),
        },
        ImmunityCase {
            case: "the modelled range floor, which is the CAPTCHA floor",
            immunity_time: 60,
            refusal: Some("WAFInvalidParameterException"),
        },
        ImmunityCase {
            case: "between the two floors",
            immunity_time: 120,
            refusal: Some("WAFInvalidParameterException"),
        },
        ImmunityCase {
            case: "one below the Challenge floor",
            immunity_time: CHALLENGE_FLOOR - 1,
            refusal: Some("WAFInvalidParameterException"),
        },
        ImmunityCase {
            case: "the Challenge floor itself",
            immunity_time: CHALLENGE_FLOOR,
            refusal: None,
        },
        ImmunityCase {
            case: "one above the Challenge floor",
            immunity_time: CHALLENGE_FLOOR + 1,
            refusal: None,
        },
        ImmunityCase {
            case: "the modelled range ceiling",
            immunity_time: 259_200,
            refusal: None,
        },
        ImmunityCase {
            case: "one past the modelled range ceiling",
            immunity_time: 259_201,
            refusal: Some("ValidationException"),
        },
    ]
}

/// A refusal this service words itself must name the challenge configuration, echo
/// the rejected value and say why, and its message must be composed from exactly
/// those three.
fn assert_challenge_refusal(body: &Value, immunity_time: i64, case: &str) {
    let field = body["Field"].as_str().unwrap_or_default().to_string();
    let parameter = body["Parameter"].as_str().unwrap_or_default().to_string();
    let reason = body["Reason"].as_str().unwrap_or_default().to_string();
    assert_eq!(
        field, FIELD_CHALLENGE_CONFIG,
        "case {case:?}: expected Field {FIELD_CHALLENGE_CONFIG}, got {field:?} in body {body}"
    );
    assert_eq!(
        parameter,
        immunity_time.to_string(),
        "case {case:?}: Parameter must echo the rejected value, got {parameter:?} in body {body}"
    );
    assert!(
        reason.contains(&CHALLENGE_FLOOR.to_string()),
        "case {case:?}: Reason must state the Challenge floor, got {reason:?}"
    );
    assert_eq!(
        error_message(body),
        format!("Error reason: {reason}, field: {field}, parameter: {parameter}"),
        "case {case:?}: the message must be composed from Reason, Field and Parameter, \
         got {body}"
    );
}

/// `CreateWebACL` refuses a below-floor CHALLENGE immunity wherever the challenge
/// configuration sits, and creates nothing when it does.
#[tokio::test]
async fn create_web_acl_enforces_the_challenge_immunity_floor() {
    // Where a `ChallengeConfig` can sit on a web ACL: "this is available at the web
    // ACL level and in each rule" (`ChallengeConfig`'s own doc).
    let placements: [Placement; 2] = [
        ("at the web ACL level", |immunity_time| {
            (immunity(immunity_time), json!([rule("plain", sqli_statement())]))
        }),
        ("on a rule", |immunity_time| {
            (
                Value::Null,
                json!([rule_with_challenge("challenged", immunity_time)]),
            )
        }),
    ];

    for (placement, build) in placements {
        for row in immunity_cases() {
            let waf = Waf::new();
            let case = format!("CreateWebACL {placement}, {}", row.case);
            let name = "challenge-acl";
            let (challenge_config, rules) = build(row.immunity_time);
            let mut request = json!({
                "Name": name,
                "Scope": "REGIONAL",
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name),
                "Rules": rules,
            });
            if !challenge_config.is_null() {
                request["ChallengeConfig"] = challenge_config;
            }

            let (status, body) = waf.call("CreateWebACL", request).await;
            match row.refusal {
                None => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: an immunity at or above the floor must be \
                         accepted, got status {status} and body {body}"
                    );
                    let id = body["Summary"]["Id"].as_str().expect("Summary.Id");
                    let got = waf
                        .ok(
                            "GetWebACL",
                            json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
                        )
                        .await;
                    let stored = if placement == "at the web ACL level" {
                        got["WebACL"]["ChallengeConfig"].clone()
                    } else {
                        got["WebACL"]["Rules"][0]["ChallengeConfig"].clone()
                    };
                    assert_eq!(
                        stored,
                        immunity(row.immunity_time),
                        "case {case:?}: the accepted immunity must read back, got {got}"
                    );
                }
                Some(expected) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected 400 {expected}, got status {status} \
                         and body {body}"
                    );
                    assert_eq!(
                        error_code(&body),
                        expected,
                        "case {case:?}: expected {expected}, got {body}"
                    );
                    if expected == "WAFInvalidParameterException" {
                        assert_challenge_refusal(&body, row.immunity_time, &case);
                    }
                    // Nothing was created: the name is still free, which a listing
                    // of the scope is what shows.
                    let listed = waf
                        .ok("ListWebACLs", json!({ "Scope": "REGIONAL" }))
                        .await;
                    assert_eq!(
                        listed["WebACLs"],
                        json!([]),
                        "case {case:?}: a refused create must store nothing, got {listed}"
                    );
                }
            }
        }
    }
}

/// `UpdateWebACL` refuses a below-floor CHALLENGE immunity, and the refused update
/// leaves the stored web ACL and its lock token exactly as they were.
#[tokio::test]
async fn update_web_acl_enforces_the_challenge_immunity_floor() {
    for row in immunity_cases() {
        let waf = Waf::new();
        let case = format!("UpdateWebACL, {}", row.case);
        let name = "challenge-acl";
        let (acl_id, _acl_arn, lock_token) =
            create_web_acl(&waf, "REGIONAL", name, json!([rule("plain", sqli_statement())])).await;
        // A web ACL saved at the floor, so every row starts from a web ACL that
        // already carries a legal challenge immunity.
        let accepted = waf
            .ok(
                "UpdateWebACL",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "Id": &acl_id,
                    "LockToken": &lock_token,
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(name),
                    "ChallengeConfig": immunity(CHALLENGE_FLOOR),
                }),
            )
            .await;
        let lock_token = accepted["NextLockToken"]
            .as_str()
            .expect("NextLockToken")
            .to_string();
        let before = waf
            .ok(
                "GetWebACL",
                json!({ "Name": name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;

        let (status, body) = waf
            .call(
                "UpdateWebACL",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "Id": &acl_id,
                    "LockToken": &lock_token,
                    "DefaultAction": { "Block": {} },
                    "VisibilityConfig": visibility(name),
                    "ChallengeConfig": immunity(row.immunity_time),
                }),
            )
            .await;
        match row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an immunity at or above the floor must be accepted, \
                     got status {status} and body {body}"
                );
                let got = waf
                    .ok(
                        "GetWebACL",
                        json!({ "Name": name, "Scope": "REGIONAL", "Id": &acl_id }),
                    )
                    .await;
                assert_eq!(
                    got["WebACL"]["ChallengeConfig"],
                    immunity(row.immunity_time),
                    "case {case:?}: the accepted immunity must read back, got {got}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 {expected}, got status {status} and body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected,
                    "case {case:?}: expected {expected}, got {body}"
                );
                if expected == "WAFInvalidParameterException" {
                    assert_challenge_refusal(&body, row.immunity_time, &case);
                }
                let after = waf
                    .ok(
                        "GetWebACL",
                        json!({ "Name": name, "Scope": "REGIONAL", "Id": &acl_id }),
                    )
                    .await;
                assert_eq!(
                    after["WebACL"], before["WebACL"],
                    "case {case:?}: a refused update must leave the web ACL untouched, \
                     got {after}"
                );
                assert_eq!(
                    after["LockToken"],
                    json!(lock_token),
                    "case {case:?}: a refused update must not rotate the lock token, \
                     got {after}"
                );
                // The token the refused call used still works, which is the other
                // side of the same statement.
                waf.ok(
                    "UpdateWebACL",
                    json!({
                        "Name": name,
                        "Scope": "REGIONAL",
                        "Id": &acl_id,
                        "LockToken": &lock_token,
                        "DefaultAction": { "Allow": {} },
                        "VisibilityConfig": visibility(name),
                    }),
                )
                .await;
            }
        }
    }
}

/// `CreateRuleGroup` enforces the same floor on its rules' challenge
/// configurations: the floor is a property of the Challenge action, not of the
/// entity that carries the rule.
#[tokio::test]
async fn create_rule_group_enforces_the_challenge_immunity_floor() {
    for row in immunity_cases() {
        let waf = Waf::new();
        let case = format!("CreateRuleGroup, {}", row.case);
        let name = "challenge-group";
        let (status, body) = waf
            .call(
                "CreateRuleGroup",
                json!({
                    "Name": name,
                    "Scope": "REGIONAL",
                    "Capacity": AMPLE_CAPACITY,
                    "VisibilityConfig": visibility(name),
                    "Rules": [rule_with_challenge("challenged", row.immunity_time)],
                }),
            )
            .await;
        match row.refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: an immunity at or above the floor must be accepted, \
                     got status {status} and body {body}"
                );
                let id = body["Summary"]["Id"].as_str().expect("Summary.Id");
                let got = waf
                    .ok(
                        "GetRuleGroup",
                        json!({ "Name": name, "Scope": "REGIONAL", "Id": id }),
                    )
                    .await;
                assert_eq!(
                    got["RuleGroup"]["Rules"][0]["ChallengeConfig"],
                    immunity(row.immunity_time),
                    "case {case:?}: the accepted immunity must read back, got {got}"
                );
            }
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected 400 {expected}, got status {status} and body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected,
                    "case {case:?}: expected {expected}, got {body}"
                );
                if expected == "WAFInvalidParameterException" {
                    assert_challenge_refusal(&body, row.immunity_time, &case);
                }
                let listed = waf
                    .ok("ListRuleGroups", json!({ "Scope": "REGIONAL" }))
                    .await;
                assert_eq!(
                    listed["RuleGroups"],
                    json!([]),
                    "case {case:?}: a refused create must store nothing, got {listed}"
                );

                // `UpdateRuleGroup` applies the same floor to the same rules: a
                // group created with a legal immunity cannot be updated past it, and
                // the refused update leaves the group and its lock token alone.
                let baseline = "baseline-group";
                let (group_id, _group_arn, group_lock) = waf
                    .ok(
                        "CreateRuleGroup",
                        json!({
                            "Name": baseline,
                            "Scope": "REGIONAL",
                            "Capacity": AMPLE_CAPACITY,
                            "VisibilityConfig": visibility(baseline),
                            "Rules": [rule_with_challenge("challenged", CHALLENGE_FLOOR)],
                        }),
                    )
                    .await["Summary"]
                    .clone()
                    .as_object()
                    .map(|summary| {
                        (
                            summary["Id"].as_str().unwrap_or_default().to_string(),
                            summary["ARN"].as_str().unwrap_or_default().to_string(),
                            summary["LockToken"].as_str().unwrap_or_default().to_string(),
                        )
                    })
                    .expect("Summary");
                let (status, body) = waf
                    .call(
                        "UpdateRuleGroup",
                        json!({
                            "Name": baseline,
                            "Scope": "REGIONAL",
                            "Id": &group_id,
                            "LockToken": &group_lock,
                            "VisibilityConfig": visibility(baseline),
                            "Rules": [rule_with_challenge("challenged", row.immunity_time)],
                        }),
                    )
                    .await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: UpdateRuleGroup must refuse the same value, got \
                     status {status} and body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected,
                    "case {case:?}: UpdateRuleGroup must answer {expected}, got {body}"
                );
                let after = waf
                    .ok(
                        "GetRuleGroup",
                        json!({ "Name": baseline, "Scope": "REGIONAL", "Id": &group_id }),
                    )
                    .await;
                assert_eq!(
                    after["RuleGroup"]["Rules"][0]["ChallengeConfig"],
                    immunity(CHALLENGE_FLOOR),
                    "case {case:?}: a refused update must leave the stored immunity \
                     alone, got {after}"
                );
                assert_eq!(
                    after["LockToken"],
                    json!(group_lock),
                    "case {case:?}: a refused update must not rotate the lock token, \
                     got {after}"
                );
            }
        }
    }
}

/// The floor is the CHALLENGE action's alone: a CAPTCHA immunity keeps the modelled
/// floor of 60, so the very values the challenge configuration refuses are accepted
/// in a `CaptchaConfig`.
///
/// This is the row that pins the two apart — a check that simply raised the member's
/// floor to 300 everywhere would pass every suite above and fail here.
#[tokio::test]
async fn the_captcha_immunity_keeps_the_modelled_floor() {
    let waf = Waf::new();
    for immunity_time in [60_i64, 120, CHALLENGE_FLOOR - 1, CHALLENGE_FLOOR, 259_200] {
        let name = format!("captcha-acl-{immunity_time}");
        let created = waf
            .ok(
                "CreateWebACL",
                json!({
                    "Name": &name,
                    "Scope": "REGIONAL",
                    "DefaultAction": { "Allow": {} },
                    "VisibilityConfig": visibility(&name),
                    "CaptchaConfig": immunity(immunity_time),
                }),
            )
            .await;
        let id = created["Summary"]["Id"].as_str().expect("Summary.Id");
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": "REGIONAL", "Id": id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["CaptchaConfig"],
            immunity(immunity_time),
            "case {immunity_time}: a CAPTCHA immunity at or above the modelled floor \
             must be stored, got {got}"
        );
    }
}
