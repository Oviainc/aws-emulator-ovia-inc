// SPDX-License-Identifier: CC0-1.0

//! The WAFv2 RegexPatternSet create/read path, end to end over the wire.
//!
//! The rule asserted here: `CreateRegexPatternSet` mints a regex pattern set from
//! the request and PERSISTS it under its scope-namespaced identity, and every later
//! read answers that stored resource — never a value recomputed from the read's own
//! request. Concretely —
//!
//! * the call "creates a RegexPatternSet, which you reference in a
//!   `RegexPatternSetReferenceStatement`", so it answers a `RegexPatternSetSummary`
//!   carrying a freshly minted `Id` and `LockToken` of the modelled form, the ARN
//!   this service mints for the set's kind and scope, and the `Description` that was
//!   stored;
//! * `RegexPatternSet$RegularExpressionList` is "the regular expression patterns in
//!   the set", so `GetRegexPatternSet` answers EXACTLY the submitted list, in the
//!   submitted order, whatever the patterns contain;
//! * `Name` is the set's identity within its scope ("you cannot change the name
//!   after you create the set"), so a second create under a name the scope already
//!   holds is refused with `WAFDuplicateItemException`, while the same name in the
//!   OTHER scope is a different resource — a scope is a namespace;
//! * a triple naming no stored set is refused with `WAFNonexistentItemException`;
//! * the ARN the create answers resolves as a `RegexPatternSetReferenceStatement`
//!   target, in the same scope only.
//!
//! Every call here goes through the same front door a real client uses (the `Waf`
//! harness in [`crate::web_acl_lifecycle`]), so what these tests observe is what an
//! SDK observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_wafv2_awswaf_20190729::state::MAX_REGEX_PATTERN_SETS_PER_SCOPE;

use crate::web_acl_lifecycle::{is_model_uuid, rule, visibility, Waf, TEST_REGION};

/// The Region a CLOUDFRONT-scoped resource's ARN carries, whatever Region the call
/// was signed for: "to work with CloudFront … use the Region endpoint us-east-1"
/// (`Scope`'s own doc).
const CLOUDFRONT_ARN_REGION: &str = "us-east-1";

/// The `(ARN region, ARN resource prefix)` a scope's ARNs carry.
fn arn_parts(scope: &str) -> (&'static str, &'static str) {
    if scope == "CLOUDFRONT" {
        (CLOUDFRONT_ARN_REGION, "global")
    } else {
        (TEST_REGION, "regional")
    }
}

/// The ARN this service mints for a regex pattern set, as
/// `arn:aws:wafv2:<region>:<account>:<global|regional>/regexpatternset/<name>/<id>`.
fn regex_pattern_set_arn(scope: &str, name: &str, id: &str) -> String {
    let (region, prefix) = arn_parts(scope);
    let account = engine::EMULATOR_ACCOUNT_ID;
    format!("arn:aws:wafv2:{region}:{account}:{prefix}/regexpatternset/{name}/{id}")
}

/// `[{ "RegexString": … }, …]` — a `RegularExpressionList` over `patterns`.
fn regular_expression_list<S: AsRef<str>>(patterns: &[S]) -> Value {
    Value::Array(
        patterns
            .iter()
            .map(|pattern| json!({ "RegexString": pattern.as_ref() }))
            .collect(),
    )
}

/// Create a regex pattern set and answer `(id, arn, lock token)`.
async fn create_regex_pattern_set<S: AsRef<str>>(
    waf: &Waf,
    scope: &str,
    name: &str,
    patterns: &[S],
    description: Option<&str>,
) -> (String, String, String) {
    let mut request = json!({
        "Name": name,
        "Scope": scope,
        "RegularExpressionList": regular_expression_list(patterns),
    });
    if let Some(description) = description {
        request["Description"] = json!(description);
    }
    let created = waf.ok("CreateRegexPatternSet", request).await;
    let summary = &created["Summary"];
    (
        summary["Id"].as_str().expect("Summary.Id").to_string(),
        summary["ARN"].as_str().expect("Summary.ARN").to_string(),
        summary["LockToken"]
            .as_str()
            .expect("Summary.LockToken")
            .to_string(),
    )
}

/// `GetRegexPatternSet` for `(scope, name, id)`, as a 200.
async fn get_regex_pattern_set(waf: &Waf, scope: &str, name: &str, id: &str) -> Value {
    waf.ok(
        "GetRegexPatternSet",
        json!({ "Name": name, "Scope": scope, "Id": id }),
    )
    .await
}

/// One create-then-read case: the scope, the patterns submitted and the
/// description, if any.
struct SetCase {
    /// What the case is called in an assertion.
    case: &'static str,
    /// The scope the set is created in.
    scope: &'static str,
    /// The patterns the create submits, in the order it submits them.
    patterns: Vec<String>,
    /// The description the create submits, or `None` when it omits the member.
    description: Option<&'static str>,
}

/// `CreateRegexPatternSet` persists the set, and `GetRegexPatternSet` answers
/// exactly what was stored.
///
/// The rows differ along everything the rule generalises over: the NUMBER of
/// patterns (one, three, four), their CONTENT (plain text, regex metacharacters and
/// escapes, non-ASCII scripts and astral-plane characters), their ORDER (the same
/// three patterns in two orders), their SIZE (a one-character pattern and a
/// 512-character one — `RegexPatternString` is `@length(1..512)`, per
/// `smithy_query(query="shape",shape="Regex")`), the SCOPE and whether the create
/// carries a `Description` at all.
#[tokio::test]
async fn a_created_regex_pattern_set_reads_back_exactly_as_submitted() {
    let cases: Vec<SetCase> = vec![
        SetCase {
            case: "one plain pattern",
            scope: "REGIONAL",
            patterns: vec!["^/admin".to_string()],
            description: Some("admin paths"),
        },
        SetCase {
            case: "three patterns, no description",
            scope: "REGIONAL",
            patterns: vec!["^/a$".to_string(), "^/b$".to_string(), "^/c$".to_string()],
            description: None,
        },
        SetCase {
            case: "the same three patterns in the opposite order",
            scope: "REGIONAL",
            patterns: vec!["^/c$".to_string(), "^/b$".to_string(), "^/a$".to_string()],
            description: Some("reordered"),
        },
        SetCase {
            case: "regex metacharacters, classes and escapes",
            scope: "REGIONAL",
            patterns: vec![
                r"(?i)\bselect\b.*\bfrom\b".to_string(),
                r"[0-9]{3}-[0-9]{2}-[0-9]{4}".to_string(),
                r"\\x41+|\t\s*$".to_string(),
            ],
            description: Some("metacharacters"),
        },
        SetCase {
            case: "non-ASCII patterns across scripts, including an astral-plane one",
            scope: "REGIONAL",
            patterns: vec![
                "привет".to_string(),
                "東京+".to_string(),
                "🙈🙉🙊".to_string(),
                "cafe\u{0301}?".to_string(),
            ],
            description: Some("unicode-patterns"),
        },
        SetCase {
            case: "a one-character pattern, the modelled length floor",
            scope: "REGIONAL",
            patterns: vec!["a".to_string()],
            description: None,
        },
        SetCase {
            case: "a 512-character pattern, the modelled length ceiling",
            scope: "REGIONAL",
            patterns: vec!["z".repeat(512)],
            description: Some("at the ceiling"),
        },
        SetCase {
            case: "a CLOUDFRONT-scoped set",
            scope: "CLOUDFRONT",
            patterns: vec!["^/global".to_string(), "\\.js$".to_string()],
            description: Some("global set"),
        },
    ];

    let waf = Waf::new();
    let mut minted: BTreeSet<String> = BTreeSet::new();
    let mut created_ids: Vec<String> = Vec::new();
    for (index, set_case) in cases.iter().enumerate() {
        let SetCase {
            case,
            scope,
            patterns,
            description,
        } = set_case;
        let name = format!("patterns-{index}");
        let (id, arn, lock) =
            create_regex_pattern_set(&waf, scope, &name, patterns, *description).await;

        assert!(
            is_model_uuid(&id),
            "case {case:?}: Summary.Id must have the modelled form, got {id:?}"
        );
        assert!(
            is_model_uuid(&lock),
            "case {case:?}: Summary.LockToken must have the modelled form, got {lock:?}"
        );
        assert_eq!(
            arn,
            regex_pattern_set_arn(scope, &name, &id),
            "case {case:?}: expected the regexpatternset ARN of this scope, got {arn:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: every create must mint a fresh Id, but {id:?} repeated"
        );
        assert!(
            minted.insert(lock.clone()),
            "case {case:?}: every create must mint a fresh LockToken, but {lock:?} repeated"
        );

        let got = get_regex_pattern_set(&waf, scope, &name, &id).await;
        let stored = &got["RegexPatternSet"];
        assert_eq!(
            stored["RegularExpressionList"],
            regular_expression_list(patterns),
            "case {case:?}: expected exactly the submitted patterns in the submitted \
             order, got {got}"
        );
        assert_eq!(
            stored["Name"],
            json!(&name),
            "case {case:?}: expected the stored name, got {got}"
        );
        assert_eq!(
            stored["Id"],
            json!(&id),
            "case {case:?}: expected the minted id, got {got}"
        );
        assert_eq!(
            stored["ARN"],
            json!(&arn),
            "case {case:?}: expected the minted ARN, got {got}"
        );
        assert_eq!(
            stored["Description"],
            json!(description.unwrap_or_default()),
            "case {case:?}: expected the submitted description (empty when the create \
             omitted it), got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&lock),
            "case {case:?}: GetRegexPatternSet must report the set's current lock \
             token, got {got}"
        );

        // A read changes nothing a later caller sees: the second read answers the
        // first one's body, token included.
        let again = get_regex_pattern_set(&waf, scope, &name, &id).await;
        assert_eq!(
            again, got,
            "case {case:?}: reading twice must answer the same stored set, got {again}"
        );
        created_ids.push(id);
    }

    // Every set created above is still readable after all the others were created:
    // one store holds them all, keyed by scope and id, so no create overwrote an
    // earlier set.
    for (index, (set_case, id)) in cases.iter().zip(&created_ids).enumerate() {
        let name = format!("patterns-{index}");
        let got = get_regex_pattern_set(&waf, set_case.scope, &name, id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&set_case.patterns),
            "case {:?}: a stored set must survive every later create, got {got}",
            set_case.case
        );
    }
}

/// A name identifies a set WITHIN its scope: a second create under a name the scope
/// already holds is refused as a duplicate and stores nothing, while the same name
/// in the other scope is a different resource.
///
/// The rows differ in what the second create carries beside the name it reuses —
/// the same patterns, different patterns, a different description, no description —
/// because it is the `(Scope, Name)` pair alone that makes it a duplicate.
#[tokio::test]
async fn a_name_is_taken_in_its_own_scope_only() {
    let waf = Waf::new();
    let first: &[&str] = &["^/first$"];
    let (regional_id, regional_arn, regional_lock) =
        create_regex_pattern_set(&waf, "REGIONAL", "shared-name", first, Some("regional")).await;

    let cases: Vec<(&str, Value)> = vec![
        (
            "the same patterns and description again",
            json!({
                "Name": "shared-name",
                "Scope": "REGIONAL",
                "Description": "regional",
                "RegularExpressionList": [{ "RegexString": "^/first$" }],
            }),
        ),
        (
            "different patterns under the taken name",
            json!({
                "Name": "shared-name",
                "Scope": "REGIONAL",
                "Description": "regional",
                "RegularExpressionList": [{ "RegexString": "^/second$" }],
            }),
        ),
        (
            "a different description under the taken name",
            json!({
                "Name": "shared-name",
                "Scope": "REGIONAL",
                "Description": "another description",
                "RegularExpressionList": [{ "RegexString": "^/first$" }],
            }),
        ),
        (
            "no description at all under the taken name",
            json!({
                "Name": "shared-name",
                "Scope": "REGIONAL",
                "RegularExpressionList": [{ "RegexString": "^/first$" }],
            }),
        ),
    ];

    for (case, request) in cases {
        waf.refused(
            "CreateRegexPatternSet",
            request,
            "WAFDuplicateItemException",
            case,
        )
        .await;
        let got = get_regex_pattern_set(&waf, "REGIONAL", "shared-name", &regional_id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(first),
            "case {case:?}: a refused create must not touch the stored set, got {got}"
        );
        assert_eq!(
            got["RegexPatternSet"]["Description"],
            json!("regional"),
            "case {case:?}: a refused create must not touch the description, got {got}"
        );
        assert_eq!(
            got["LockToken"],
            json!(&regional_lock),
            "case {case:?}: a refused create must not rotate the lock token, got {got}"
        );
    }

    // The same name in the other scope is a different resource, with its own id,
    // ARN and contents.
    let cloudfront: &[&str] = &["^/cloudfront$", "^/edge$"];
    let (cloudfront_id, cloudfront_arn, _cloudfront_lock) =
        create_regex_pattern_set(&waf, "CLOUDFRONT", "shared-name", cloudfront, None).await;
    assert_ne!(
        cloudfront_id, regional_id,
        "the same name in the other scope must be a different resource"
    );
    assert_ne!(
        cloudfront_arn, regional_arn,
        "the same name in the other scope must carry its own ARN"
    );
    let got = get_regex_pattern_set(&waf, "CLOUDFRONT", "shared-name", &cloudfront_id).await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(cloudfront),
        "the CLOUDFRONT set must answer its own patterns, got {got}"
    );
    let got = get_regex_pattern_set(&waf, "REGIONAL", "shared-name", &regional_id).await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(first),
        "the REGIONAL set must be untouched by the CLOUDFRONT one, got {got}"
    );
}

/// `GetRegexPatternSet` answers only the set the whole `(Scope, Name, Id)` triple
/// describes; any other triple names no resource and is refused with
/// `WAFNonexistentItemException`.
///
/// One row per way the triple can miss: a name that names no set, a well-formed id
/// that was never minted, the id of ANOTHER set under this name, the name of
/// another set under this id, and the right pair in the wrong scope.
#[tokio::test]
async fn a_get_that_names_no_stored_set_is_refused() {
    let waf = Waf::new();
    let (id, _arn, _lock) = create_regex_pattern_set(
        &waf,
        "REGIONAL",
        "readable-set",
        &["^/readable$"],
        Some("readable"),
    )
    .await;
    let (other_id, _other_arn, _other_lock) =
        create_regex_pattern_set(&waf, "REGIONAL", "other-set", &["^/other$"], None).await;
    let never_minted = "99999999-8888-7777-6666-555555555555";

    let cases: Vec<(&str, Value)> = vec![
        (
            "a name that names no set",
            json!({ "Name": "no-such-set", "Scope": "REGIONAL", "Id": &id }),
        ),
        (
            "an id that was never minted",
            json!({ "Name": "readable-set", "Scope": "REGIONAL", "Id": never_minted }),
        ),
        (
            "another set's id under this name",
            json!({ "Name": "readable-set", "Scope": "REGIONAL", "Id": &other_id }),
        ),
        (
            "another set's name under this id",
            json!({ "Name": "other-set", "Scope": "REGIONAL", "Id": &id }),
        ),
        (
            "the right name and id in the other scope",
            json!({ "Name": "readable-set", "Scope": "CLOUDFRONT", "Id": &id }),
        ),
    ];

    for (case, request) in cases {
        waf.refused(
            "GetRegexPatternSet",
            request,
            "WAFNonexistentItemException",
            case,
        )
        .await;
    }

    // The refusals left the readable set readable.
    let got = get_regex_pattern_set(&waf, "REGIONAL", "readable-set", &id).await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(&["^/readable$"]),
        "a refused read must leave the stored set readable, got {got}"
    );
}

/// The WCUs one `RegexPatternSetReferenceStatement` costs a web ACL, as
/// [`service_com_amazonaws_wafv2_awswaf_20190729::state`] prices it.
const REGEX_REFERENCE_WCU: i64 = 25;

/// A set this service holds is what a `RegexPatternSetReferenceStatement` may name:
/// the ARN the create answered resolves, in the same scope only, and an ARN naming
/// no stored set is still refused.
///
/// This is the second read path onto the store, so it also proves the create wrote
/// the ARN index and not just the by-id map.
#[tokio::test]
async fn a_stored_set_is_what_a_rule_reference_may_name() {
    let waf = Waf::new();
    let (_regional_id, regional_arn, _regional_lock) = create_regex_pattern_set(
        &waf,
        "REGIONAL",
        "referenced-patterns",
        &["^/referenced$"],
        None,
    )
    .await;
    let (_edge_id, edge_arn, _edge_lock) = create_regex_pattern_set(
        &waf,
        "CLOUDFRONT",
        "edge-patterns",
        &["^/edge$"],
        None,
    )
    .await;
    let absent_arn = format!(
        "arn:aws:wafv2:{TEST_REGION}:{}:regional/regexpatternset/absent-set/\
         33333333-4444-5555-6666-777777777777",
        engine::EMULATOR_ACCOUNT_ID
    );

    // (case, the ARN the rule references, accepted?)
    let cases: Vec<(&str, &str, bool)> = vec![
        (
            "a set stored in this scope",
            regional_arn.as_str(),
            true,
        ),
        (
            "a set stored in the other scope",
            edge_arn.as_str(),
            false,
        ),
        (
            "a well-formed ARN naming no stored set",
            absent_arn.as_str(),
            false,
        ),
    ];

    for (index, (case, arn, accepted)) in cases.into_iter().enumerate() {
        let name = format!("regex-reference-acl-{index}");
        let request = json!({
            "Name": &name,
            "Scope": "REGIONAL",
            "DefaultAction": { "Allow": {} },
            "VisibilityConfig": visibility(&name),
            "Rules": [rule(
                "match-patterns",
                json!({
                    "RegexPatternSetReferenceStatement": {
                        "ARN": arn,
                        "FieldToMatch": { "QueryString": {} },
                        "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                    }
                }),
            )],
        });
        if !accepted {
            waf.refused(
                "CreateWebACL",
                request,
                "WAFNonexistentItemException",
                case,
            )
            .await;
            continue;
        }
        let created = waf.ok("CreateWebACL", request).await;
        let acl_id = created["Summary"]["Id"]
            .as_str()
            .expect("Summary.Id")
            .to_string();
        let got = waf
            .ok(
                "GetWebACL",
                json!({ "Name": &name, "Scope": "REGIONAL", "Id": &acl_id }),
            )
            .await;
        assert_eq!(
            got["WebACL"]["Rules"][0]["Statement"]["RegexPatternSetReferenceStatement"]["ARN"],
            json!(arn),
            "case {case:?}: the stored rule must keep the referenced ARN, got {got}"
        );
        assert_eq!(
            got["WebACL"]["Capacity"],
            json!(REGEX_REFERENCE_WCU),
            "case {case:?}: expected the regex-reference WCU cost, got {got}"
        );
    }
}

/// The store holds at most [`MAX_REGEX_PATTERN_SETS_PER_SCOPE`] sets per scope:
/// the quota'th create still succeeds, the next is refused with
/// `WAFLimitsExceededException`, and the OTHER scope is unaffected — the bound is
/// per scope, like every other WAFv2 entity bound in this service.
#[tokio::test]
async fn the_regex_pattern_set_quota_is_enforced_per_scope() {
    let waf = Waf::new();
    let mut ids: Vec<String> = Vec::new();
    for index in 0..MAX_REGEX_PATTERN_SETS_PER_SCOPE {
        let name = format!("quota-set-{index}");
        let (id, _arn, _lock) =
            create_regex_pattern_set(&waf, "REGIONAL", &name, &["^/quota$"], None).await;
        ids.push(id);
    }

    let case = "one set past the per-scope quota";
    waf.refused(
        "CreateRegexPatternSet",
        json!({
            "Name": "quota-set-over",
            "Scope": "REGIONAL",
            "RegularExpressionList": [{ "RegexString": "^/over$" }],
        }),
        "WAFLimitsExceededException",
        case,
    )
    .await;

    // The refusal stored nothing, and every set already in the scope is readable.
    for (index, id) in ids.iter().enumerate() {
        let name = format!("quota-set-{index}");
        let got = get_regex_pattern_set(&waf, "REGIONAL", &name, id).await;
        assert_eq!(
            got["RegexPatternSet"]["RegularExpressionList"],
            regular_expression_list(&["^/quota$"]),
            "case {case:?}: the sets inside the quota must stay readable, got {got}"
        );
    }

    // The other scope's budget is its own.
    let (edge_id, _edge_arn, _edge_lock) = create_regex_pattern_set(
        &waf,
        "CLOUDFRONT",
        "quota-set-over",
        &["^/over$"],
        None,
    )
    .await;
    let got = get_regex_pattern_set(&waf, "CLOUDFRONT", "quota-set-over", &edge_id).await;
    assert_eq!(
        got["RegexPatternSet"]["RegularExpressionList"],
        regular_expression_list(&["^/over$"]),
        "the quota is per scope, so the other scope still accepts a create, got {got}"
    );
}
