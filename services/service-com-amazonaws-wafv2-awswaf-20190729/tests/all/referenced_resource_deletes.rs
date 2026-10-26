// SPDX-License-Identifier: CC0-1.0

//! Deleting a WAFv2 resource that a stored rule tree still references, end to end
//! over the wire.
//!
//! The rule these suites assert is: a resource is "being used by another resource"
//! as soon as ANY stored rule tree names it, so the delete is refused with 400
//! `WAFAssociatedItemException` and the resource survives — and this model stores
//! rule trees in TWO kinds of entity, because `Rule` is "a single rule, which you
//! can use in a WebACL or a RuleGroup", so a rule group's rules hold a resource
//! exactly as a web ACL's do. Once the holder is gone the delete succeeds.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the rows differ in WHICH kind of entity holds the reference, which KIND of
//! resource is referenced, and where in the statement tree the reference sits. The
//! harness is the one in [`crate::web_acl_lifecycle`]: every call goes through the
//! router's front door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::regex_pattern_set_lifecycle::create_regex_pattern_set;
use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, rule, sqli_statement, Waf};

/// A declared capacity comfortably above what any rule list below costs, so a row
/// is never refused for its capacity instead of for the reference under test.
const AMPLE_CAPACITY: i64 = 500;

/// The kind of resource a row references, with the operations that create and
/// delete it.
#[derive(Clone, Copy, Debug)]
enum Referenced {
    /// An `IPSet`, referenced by an `IPSetReferenceStatement`.
    IpSet,
    /// A `RegexPatternSet`, referenced by a `RegexPatternSetReferenceStatement`.
    RegexPatternSet,
}

impl Referenced {
    /// The `Delete…` operation for this kind.
    fn delete_operation(self) -> &'static str {
        match self {
            Self::IpSet => "DeleteIPSet",
            Self::RegexPatternSet => "DeleteRegexPatternSet",
        }
    }

    /// The `Get…` operation for this kind.
    fn get_operation(self) -> &'static str {
        match self {
            Self::IpSet => "GetIPSet",
            Self::RegexPatternSet => "GetRegexPatternSet",
        }
    }

    /// The reference statement that names `arn`.
    fn reference(self, arn: &str) -> Value {
        match self {
            Self::IpSet => json!({ "IPSetReferenceStatement": { "ARN": arn } }),
            Self::RegexPatternSet => json!({
                "RegexPatternSetReferenceStatement": {
                    "ARN": arn,
                    "FieldToMatch": { "QueryString": {} },
                    "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                }
            }),
        }
    }

    /// Create one in `scope` and answer `(id, arn, lock token)`.
    async fn create(self, waf: &Waf, scope: &str, name: &str) -> (String, String, String) {
        match self {
            Self::IpSet => create_ip_set(waf, scope, name).await,
            Self::RegexPatternSet => {
                create_regex_pattern_set(waf, scope, name, &["^/admin"], None).await
            }
        }
    }
}

/// Which kind of entity holds the referencing rule tree.
#[derive(Clone, Copy, Debug)]
enum Holder {
    /// A web ACL, deleted with `DeleteWebACL`.
    WebAcl,
    /// A rule group, deleted with `DeleteRuleGroup`.
    RuleGroup,
}

impl Holder {
    /// The `Delete…` operation for this kind.
    fn delete_operation(self) -> &'static str {
        match self {
            Self::WebAcl => "DeleteWebACL",
            Self::RuleGroup => "DeleteRuleGroup",
        }
    }

    /// The name the suite gives this holder.
    fn name(self) -> &'static str {
        match self {
            Self::WebAcl => "holding-acl",
            Self::RuleGroup => "holding-group",
        }
    }
}

/// One way of nesting a reference statement inside a rule's statement tree.
type Nesting = (&'static str, fn(Value) -> Value);

/// Every place a reference statement can sit in a rule's tree: at the top level,
/// or inside any of the statements that nest another one.
fn nestings() -> &'static [Nesting] {
    &[
        ("top level", |reference| reference),
        ("inside AndStatement", |reference| {
            json!({ "AndStatement": { "Statements": [reference, sqli_statement()] } })
        }),
        ("inside OrStatement", |reference| {
            json!({ "OrStatement": { "Statements": [sqli_statement(), reference] } })
        }),
        ("inside NotStatement", |reference| {
            json!({ "NotStatement": { "Statement": reference } })
        }),
        ("inside a rate-based scope-down", |reference| {
            json!({
                "RateBasedStatement": {
                    "Limit": 1000,
                    "AggregateKeyType": "IP",
                    "ScopeDownStatement": reference,
                }
            })
        }),
    ]
}

/// A resource a stored rule tree references cannot be deleted, whichever kind of
/// entity holds that tree and wherever in it the reference sits.
///
/// The rows cross the referenced KIND (an IP set, a regex pattern set) with the
/// HOLDER (a web ACL, a rule group) and with every nesting of the reference
/// statement, so no row can pass by the same path as the recorded one — which
/// referenced an IP set from a web ACL.
#[tokio::test]
async fn a_resource_any_stored_rule_tree_references_cannot_be_deleted() {
    for referenced in [Referenced::IpSet, Referenced::RegexPatternSet] {
        for holder in [Holder::WebAcl, Holder::RuleGroup] {
            for (nesting, nest) in nestings() {
                let case = format!("{referenced:?} referenced by {holder:?}, {nesting}");
                let waf = Waf::new();
                let (resource_id, resource_arn, resource_lock) =
                    referenced.create(&waf, "REGIONAL", "held-resource").await;
                let rules = json!([rule("holds-a-reference", nest(referenced.reference(&resource_arn)))]);
                let delete_resource = json!({
                    "Name": "held-resource",
                    "Scope": "REGIONAL",
                    "Id": &resource_id,
                    "LockToken": &resource_lock,
                });

                let (holder_id, holder_lock) = match holder {
                    Holder::WebAcl => {
                        let (id, _arn, lock) =
                            create_web_acl(&waf, "REGIONAL", holder.name(), rules).await;
                        (id, lock)
                    }
                    Holder::RuleGroup => {
                        let (id, _arn, lock) = create_rule_group(
                            &waf,
                            "REGIONAL",
                            holder.name(),
                            AMPLE_CAPACITY,
                            rules,
                        )
                        .await;
                        (id, lock)
                    }
                };

                waf.refused(
                    referenced.delete_operation(),
                    delete_resource.clone(),
                    "WAFAssociatedItemException",
                    &case,
                )
                .await;
                // The refusal left the resource exactly where it was: it still reads
                // back under the same triple, and its lock token did not rotate.
                waf.ok(
                    referenced.get_operation(),
                    json!({ "Name": "held-resource", "Scope": "REGIONAL", "Id": &resource_id }),
                )
                .await;

                // A stale token is refused for the LOCK, not for the association, so
                // the two refusals cannot be confused with each other.
                waf.refused(
                    referenced.delete_operation(),
                    json!({
                        "Name": "held-resource",
                        "Scope": "REGIONAL",
                        "Id": &resource_id,
                        "LockToken": "00000000-0000-4000-8000-000000000000",
                    }),
                    "WAFOptimisticLockException",
                    &case,
                )
                .await;

                // Take the holder away, and the delete that was refused succeeds.
                waf.ok(
                    holder.delete_operation(),
                    json!({
                        "Name": holder.name(),
                        "Scope": "REGIONAL",
                        "Id": &holder_id,
                        "LockToken": &holder_lock,
                    }),
                )
                .await;
                // Nothing references it any more, so the delete that was refused now
                // succeeds and the resource is gone.
                waf.ok(referenced.delete_operation(), delete_resource).await;
                waf.refused(
                    referenced.get_operation(),
                    json!({ "Name": "held-resource", "Scope": "REGIONAL", "Id": &resource_id }),
                    "WAFNonexistentItemException",
                    &case,
                )
                .await;
            }
        }
    }
}

/// A resource NO rule tree references deletes, and a rule group a web ACL still
/// RUNS cannot be deleted: a rule-group reference is a reference like any other.
#[tokio::test]
async fn a_rule_group_a_web_acl_runs_cannot_be_deleted() {
    let waf = Waf::new();
    let (group_id, group_arn, group_lock) = create_rule_group(
        &waf,
        "REGIONAL",
        "shared-group",
        AMPLE_CAPACITY,
        json!([rule("inspect", sqli_statement())]),
    )
    .await;
    // A rule group is run by a rule's own top-level statement, which carries an
    // `OverrideAction` rather than an `Action`.
    let runner = json!([{
        "Name": "run-the-group",
        "Priority": 0,
        "Statement": { "RuleGroupReferenceStatement": { "ARN": &group_arn } },
        "OverrideAction": { "None": {} },
        "VisibilityConfig": {
            "SampledRequestsEnabled": true,
            "CloudWatchMetricsEnabled": true,
            "MetricName": "run-the-group",
        },
    }]);
    let (acl_id, _acl_arn, acl_lock) =
        create_web_acl(&waf, "REGIONAL", "running-acl", runner).await;

    let delete_group = json!({
        "Name": "shared-group",
        "Scope": "REGIONAL",
        "Id": &group_id,
        "LockToken": &group_lock,
    });
    waf.refused(
        "DeleteRuleGroup",
        delete_group.clone(),
        "WAFAssociatedItemException",
        "a rule group a web ACL runs",
    )
    .await;
    waf.ok(
        "GetRuleGroup",
        json!({ "Name": "shared-group", "Scope": "REGIONAL", "Id": &group_id }),
    )
    .await;

    waf.ok(
        "DeleteWebACL",
        json!({
            "Name": "running-acl",
            "Scope": "REGIONAL",
            "Id": &acl_id,
            "LockToken": &acl_lock,
        }),
    )
    .await;
    waf.ok("DeleteRuleGroup", delete_group).await;
    waf.refused(
        "GetRuleGroup",
        json!({ "Name": "shared-group", "Scope": "REGIONAL", "Id": &group_id }),
        "WAFNonexistentItemException",
        "a rule group after it was deleted",
    )
    .await;
}

/// A resource NO rule tree references deletes, and a reference in ANOTHER scope is
/// no reference at all: a scope is a namespace, so the two cannot hold each other.
#[tokio::test]
async fn an_unreferenced_resource_deletes() {
    for referenced in [Referenced::IpSet, Referenced::RegexPatternSet] {
        let case = format!("{referenced:?} with no reference to it");
        let waf = Waf::new();
        let (resource_id, _arn, resource_lock) =
            referenced.create(&waf, "REGIONAL", "free-resource").await;
        // A rule group in the same scope that references NOTHING, so the walk has a
        // rule tree to look at and still finds no reference.
        create_rule_group(
            &waf,
            "REGIONAL",
            "unrelated-group",
            AMPLE_CAPACITY,
            json!([rule("unrelated", sqli_statement())]),
        )
        .await;

        waf.ok(
            referenced.delete_operation(),
            json!({
                "Name": "free-resource",
                "Scope": "REGIONAL",
                "Id": &resource_id,
                "LockToken": &resource_lock,
            }),
        )
        .await;
        waf.refused(
            referenced.get_operation(),
            json!({ "Name": "free-resource", "Scope": "REGIONAL", "Id": &resource_id }),
            "WAFNonexistentItemException",
            &case,
        )
        .await;
    }
}
