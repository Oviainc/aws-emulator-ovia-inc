// SPDX-License-Identifier: CC0-1.0

//! Sharing a WAFv2 rule group: `PutPermissionPolicy`, `GetPermissionPolicy` and
//! `DeletePermissionPolicy`, end to end over the wire.
//!
//! The rule these suites assert is: a permission policy within the restrictions
//! `PutPermissionPolicy` documents is STORED on the rule group its `ResourceArn`
//! resolves to and read back verbatim, the last one attached being the one that is
//! carried and a delete leaving the rule group sharing nothing; a document outside
//! those restrictions is refused with `WAFInvalidPermissionPolicyException` and
//! changes nothing; and an address that resolves to no rule group of this account is
//! refused without storing anything — malformed as the parameter failure, well formed
//! as the nonexistent-item failure.
//!
//! Each suite drives that over a TABLE spanning the class rather than one request:
//! the accepted rows differ in which of the documented actions they grant and in
//! what order, in how many statements they spread them over, in the form of their
//! `Principal`, in the whitespace of the document text and in the scope of the rule
//! group; the refused rows differ in WHICH restriction they break. The harness is the
//! one in [`crate::web_acl_lifecycle`]: every call goes through the router's front
//! door, so what these tests observe is what an SDK observes.

use serde_json::{json, Value};

use crate::rule_group_lifecycle::create_rule_group;
use crate::web_acl_lifecycle::{create_ip_set, create_web_acl, Waf, TEST_REGION};

/// The three actions a permission policy has to grant, and the one it may grant
/// besides them.
///
/// Source: `PutPermissionPolicyRequest$Policy`'s own documentation
/// (`smithy_query(query="doc",shape="PutPermissionPolicyRequest",member="Policy")`) —
/// "Action must specify wafv2:CreateWebACL, wafv2:UpdateWebACL, and
/// wafv2:PutFirewallManagerRuleGroups and may optionally specify
/// wafv2:GetRuleGroup. WAF rejects any extra actions or wildcard actions in the
/// policy." Spelled out here as literals, independently of the service's own
/// constants, so these suites pin the catalogue and not merely its reuse.
pub(crate) const CREATE_WEB_ACL: &str = "wafv2:CreateWebACL";
/// See [`CREATE_WEB_ACL`].
pub(crate) const UPDATE_WEB_ACL: &str = "wafv2:UpdateWebACL";
/// See [`CREATE_WEB_ACL`].
pub(crate) const PUT_FIREWALL_MANAGER_RULE_GROUPS: &str = "wafv2:PutFirewallManagerRuleGroups";
/// The one OPTIONAL action. See [`CREATE_WEB_ACL`].
pub(crate) const GET_RULE_GROUP: &str = "wafv2:GetRuleGroup";

/// The IAM policy language version a permission policy must declare ("The policy
/// must be composed using IAM Policy version 2012-10-17", the same doc).
pub(crate) const POLICY_VERSION: &str = "2012-10-17";

/// The account a shared rule group's policy names as its principal.
pub(crate) const OTHER_ACCOUNT: &str = "arn:aws:iam::222222222222:root";

/// A second account, for a policy naming more than one principal.
const THIRD_ACCOUNT: &str = "arn:aws:iam::333333333333:root";

/// `WAFInvalidPermissionPolicyException`'s sentence, as recorded from WAF — one
/// sentence for the exception TYPE, carrying nothing of the request, the way the
/// sibling suite `crate::exception_messages` pins every other WAFv2 exception. It is
/// the wire wording, not the description the shape's documentation trait gives the
/// error (see `crate::invalid_permission_policy_message`).
const INVALID_PERMISSION_POLICY: &str = "The policy specification doesn't meet the \
     requirements to be attached to an AWS WAF resource.";

/// `WAFNonexistentItemException`'s sentence, as recorded from WAF (the wording the
/// sibling suite `crate::exception_messages` pins for every operation).
const NONEXISTENT_ITEM: &str =
    "AWS WAF couldn’t perform the operation because your resource doesn’t exist.";

/// A string long enough to satisfy `ResourceArn`'s `@length(20..2048)` that is not
/// an ARN at all, so the modelled constraints admit it and the handler is what
/// refuses it.
const MALFORMED_ARN: &str = "this-is-not-an-arn-but-it-is-long-enough";

/// A well-formed wafv2 rule group ARN that no create minted.
const MISSING_RULE_GROUP_ARN: &str = "arn:aws:wafv2:us-west-2:000000000000:regional/\
     rulegroup/probe-absent-group/00000000-1111-2222-3333-444444444444";

/// The awsJson error body's message member, whichever casing the shape models.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// An `Allow` statement granting `actions` to `principal`.
pub(crate) fn allow(actions: Value, principal: Value) -> Value {
    json!({ "Effect": "Allow", "Principal": principal, "Action": actions })
}

/// A policy document, rendered the way an SDK caller would hand it over.
pub(crate) fn policy(statements: Value) -> String {
    json!({ "Version": POLICY_VERSION, "Statement": statements }).to_string()
}

/// The three required actions, in the order the documentation lists them.
pub(crate) fn required_actions() -> Value {
    json!([
        CREATE_WEB_ACL,
        UPDATE_WEB_ACL,
        PUT_FIREWALL_MANAGER_RULE_GROUPS
    ])
}

/// A policy every suite here can use as a baseline: the required actions granted to
/// one account.
fn baseline_policy() -> String {
    policy(json!([allow(required_actions(), json!(OTHER_ACCOUNT))]))
}

/// `GetPermissionPolicy` for `arn`, which must answer `expected`.
async fn assert_stored_policy(waf: &Waf, arn: &str, expected: &str, case: &str) {
    let read = waf
        .ok("GetPermissionPolicy", json!({ "ResourceArn": arn }))
        .await;
    assert_eq!(
        read["Policy"].as_str(),
        Some(expected),
        "case {case:?}: GetPermissionPolicy must answer the stored policy text, got {read}"
    );
}

/// `GetPermissionPolicy` for `arn`, which must refuse: no policy is attached.
async fn assert_no_stored_policy(waf: &Waf, arn: &str, case: &str) {
    let refusal = waf
        .refused(
            "GetPermissionPolicy",
            json!({ "ResourceArn": arn }),
            "WAFNonexistentItemException",
            case,
        )
        .await;
    assert_eq!(
        error_message(&refusal),
        NONEXISTENT_ITEM,
        "case {case:?}: the refusal must carry the nonexistent-item sentence, got {refusal}"
    );
}

/// One row of the accepted-policy table.
struct AcceptedPolicy {
    case: &'static str,
    scope: &'static str,
    policy: String,
}

/// Every policy form within the documented restrictions is stored on the rule group
/// its ARN names and read back verbatim.
///
/// The rows differ along what the rule generalises over: which documented actions
/// are granted and in what order, how many statements they are spread over, the form
/// of the `Principal` (a string, an `AWS` map, a list of principals, a service), the
/// presence of a `Sid`, the whitespace of the document text, and the scope of the
/// rule group the policy is attached to.
#[tokio::test]
async fn a_policy_within_the_restrictions_is_stored_and_read_back() {
    let waf = Waf::new();

    let rows = vec![
        AcceptedPolicy {
            case: "the required actions as a list, a string principal",
            scope: "REGIONAL",
            policy: baseline_policy(),
        },
        AcceptedPolicy {
            case: "the required actions plus the optional one",
            scope: "CLOUDFRONT",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    GET_RULE_GROUP
                ]),
                json!({ "AWS": OTHER_ACCOUNT })
            )])),
        },
        AcceptedPolicy {
            case: "the actions in another order",
            scope: "REGIONAL",
            policy: policy(json!([allow(
                json!([
                    GET_RULE_GROUP,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    UPDATE_WEB_ACL,
                    CREATE_WEB_ACL
                ]),
                json!({ "AWS": OTHER_ACCOUNT })
            )])),
        },
        AcceptedPolicy {
            case: "two principals in one statement",
            scope: "REGIONAL",
            policy: policy(json!([allow(
                required_actions(),
                json!({ "AWS": [OTHER_ACCOUNT, THIRD_ACCOUNT] })
            )])),
        },
        AcceptedPolicy {
            case: "a service principal",
            scope: "CLOUDFRONT",
            policy: policy(json!([allow(
                required_actions(),
                json!({ "Service": "firewallmanager.amazonaws.com" })
            )])),
        },
        AcceptedPolicy {
            case: "the required actions spread over two statements",
            scope: "CLOUDFRONT",
            policy: policy(json!([
                allow(
                    json!([CREATE_WEB_ACL, UPDATE_WEB_ACL]),
                    json!(OTHER_ACCOUNT)
                ),
                allow(
                    json!([PUT_FIREWALL_MANAGER_RULE_GROUPS, GET_RULE_GROUP]),
                    json!({ "AWS": THIRD_ACCOUNT })
                ),
            ])),
        },
        AcceptedPolicy {
            case: "one action per statement, each with a Sid",
            scope: "REGIONAL",
            policy: policy(json!([
                { "Sid": "create", "Effect": "Allow", "Principal": OTHER_ACCOUNT, "Action": CREATE_WEB_ACL },
                { "Sid": "update", "Effect": "Allow", "Principal": OTHER_ACCOUNT, "Action": UPDATE_WEB_ACL },
                { "Sid": "fms", "Effect": "Allow", "Principal": OTHER_ACCOUNT, "Action": PUT_FIREWALL_MANAGER_RULE_GROUPS },
            ])),
        },
        AcceptedPolicy {
            case: "a lone statement object rather than a list",
            scope: "REGIONAL",
            policy: policy(allow(required_actions(), json!(OTHER_ACCOUNT))),
        },
        AcceptedPolicy {
            case: "a document the caller formatted over several lines",
            scope: "REGIONAL",
            policy: format!(
                "{{\n  \"Version\": \"{POLICY_VERSION}\",\n  \"Statement\": [\n    {{\n      \
                 \"Effect\": \"Allow\",\n      \"Principal\": {{ \"AWS\": \"{OTHER_ACCOUNT}\" }},\n      \
                 \"Action\": [\n        \"{CREATE_WEB_ACL}\",\n        \"{UPDATE_WEB_ACL}\",\n        \
                 \"{PUT_FIREWALL_MANAGER_RULE_GROUPS}\"\n      ]\n    }}\n  ]\n}}"
            ),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let name = format!("shared-group-{index}");
        let (_id, arn, _lock) = create_rule_group(&waf, row.scope, &name, 10, json!([])).await;

        // Nothing is shared until a policy is attached, whatever the rule group
        // itself carries.
        assert_no_stored_policy(&waf, &arn, row.case).await;

        let attached = waf
            .ok(
                "PutPermissionPolicy",
                json!({ "ResourceArn": &arn, "Policy": &row.policy }),
            )
            .await;
        assert!(
            attached
                .as_object()
                .is_none_or(|members| members.is_empty()),
            "case {:?}: PutPermissionPolicy models an empty response, got {attached}",
            row.case
        );

        assert_stored_policy(&waf, &arn, &row.policy, row.case).await;
    }
}

/// Attaching a policy REPLACES the one the rule group carried: "you can attach only
/// one policy with each PutPermissionPolicy request", and the rule group is addressed
/// by its own ARN, so the last document attached is the one that is read back.
#[tokio::test]
async fn attaching_a_policy_replaces_the_one_the_rule_group_carried() {
    let waf = Waf::new();
    let (_id, arn, _lock) = create_rule_group(&waf, "REGIONAL", "replaced-policy", 10, json!([]))
        .await;

    let sequence = [
        (
            "the required actions",
            policy(json!([allow(required_actions(), json!(OTHER_ACCOUNT))])),
        ),
        (
            "the optional action added",
            policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    GET_RULE_GROUP
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        ),
        (
            "another principal",
            policy(json!([allow(
                required_actions(),
                json!({ "AWS": [OTHER_ACCOUNT, THIRD_ACCOUNT] })
            )])),
        ),
        (
            "back to the first form",
            policy(json!([allow(required_actions(), json!(OTHER_ACCOUNT))])),
        ),
    ];

    for (case, document) in &sequence {
        waf.ok(
            "PutPermissionPolicy",
            json!({ "ResourceArn": &arn, "Policy": document }),
        )
        .await;
        assert_stored_policy(&waf, &arn, document, case).await;
    }
}

/// One row of the refused-policy table: a document outside ONE of the documented
/// restrictions.
struct RefusedPolicy {
    case: &'static str,
    policy: String,
}

/// A document outside the documented restrictions is refused with
/// `WAFInvalidPermissionPolicyException`, and the refusal stores nothing: a rule group
/// that carried a policy still carries exactly the one it had, and one that carried
/// none still carries none.
///
/// The rows differ in WHICH restriction is broken — the policy version, the presence
/// of `Effect`/`Action`/`Principal`, the `Effect` value, an action outside the
/// documented list (including every form of wildcard), a required action missing, a
/// `Resource`, a negated member, and text that is not a policy document at all.
#[tokio::test]
async fn a_policy_outside_the_restrictions_is_refused_and_stores_nothing() {
    let waf = Waf::new();
    let baseline = baseline_policy();

    let rows = vec![
        RefusedPolicy {
            case: "a wildcard action for the whole service",
            policy: policy(json!([allow(json!("wafv2:*"), json!(OTHER_ACCOUNT))])),
        },
        RefusedPolicy {
            case: "a bare wildcard action",
            policy: policy(json!([allow(json!("*"), json!(OTHER_ACCOUNT))])),
        },
        RefusedPolicy {
            case: "a wildcard beside the required actions",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    "wafv2:Get*"
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedPolicy {
            case: "an extra action outside the documented list",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    "wafv2:DeleteRuleGroup"
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedPolicy {
            case: "an action of another service",
            policy: policy(json!([allow(
                json!([
                    CREATE_WEB_ACL,
                    UPDATE_WEB_ACL,
                    PUT_FIREWALL_MANAGER_RULE_GROUPS,
                    "waf:UpdateWebACL"
                ]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedPolicy {
            case: "only two of the three required actions",
            policy: policy(json!([allow(
                json!([CREATE_WEB_ACL, UPDATE_WEB_ACL]),
                json!(OTHER_ACCOUNT)
            )])),
        },
        RefusedPolicy {
            case: "the optional action alone",
            policy: policy(json!([allow(json!(GET_RULE_GROUP), json!(OTHER_ACCOUNT))])),
        },
        RefusedPolicy {
            case: "an empty action list",
            policy: policy(json!([allow(json!([]), json!(OTHER_ACCOUNT))])),
        },
        RefusedPolicy {
            case: "no Action at all",
            policy: policy(json!([
                { "Effect": "Allow", "Principal": OTHER_ACCOUNT }
            ])),
        },
        RefusedPolicy {
            case: "an Effect of Deny",
            policy: policy(json!([
                { "Effect": "Deny", "Principal": OTHER_ACCOUNT, "Action": required_actions() }
            ])),
        },
        RefusedPolicy {
            case: "no Effect",
            policy: policy(json!([
                { "Principal": OTHER_ACCOUNT, "Action": required_actions() }
            ])),
        },
        RefusedPolicy {
            case: "no Principal",
            policy: policy(json!([
                { "Effect": "Allow", "Action": required_actions() }
            ])),
        },
        RefusedPolicy {
            case: "an empty Principal map",
            policy: policy(json!([allow(required_actions(), json!({}))])),
        },
        RefusedPolicy {
            case: "a blank principal",
            policy: policy(json!([allow(required_actions(), json!("   "))])),
        },
        RefusedPolicy {
            case: "a Resource parameter",
            policy: policy(json!([
                {
                    "Effect": "Allow",
                    "Principal": OTHER_ACCOUNT,
                    "Action": required_actions(),
                    "Resource": "*",
                }
            ])),
        },
        RefusedPolicy {
            case: "a NotResource parameter",
            policy: policy(json!([
                {
                    "Effect": "Allow",
                    "Principal": OTHER_ACCOUNT,
                    "Action": required_actions(),
                    "NotResource": "arn:aws:wafv2:us-west-2:000000000000:regional/rulegroup/x/y",
                }
            ])),
        },
        RefusedPolicy {
            case: "NotAction in place of Action",
            policy: policy(json!([
                { "Effect": "Allow", "Principal": OTHER_ACCOUNT, "NotAction": "wafv2:DeleteRuleGroup" }
            ])),
        },
        RefusedPolicy {
            case: "NotPrincipal in place of Principal",
            policy: policy(json!([
                { "Effect": "Allow", "NotPrincipal": OTHER_ACCOUNT, "Action": required_actions() }
            ])),
        },
        RefusedPolicy {
            case: "one statement of two outside the restrictions",
            policy: policy(json!([
                allow(required_actions(), json!(OTHER_ACCOUNT)),
                { "Effect": "Deny", "Principal": THIRD_ACCOUNT, "Action": GET_RULE_GROUP },
            ])),
        },
        RefusedPolicy {
            case: "an older policy language version",
            policy: json!({
                "Version": "2008-10-17",
                "Statement": [allow(required_actions(), json!(OTHER_ACCOUNT))],
            })
            .to_string(),
        },
        RefusedPolicy {
            case: "no Version",
            policy: json!({ "Statement": [allow(required_actions(), json!(OTHER_ACCOUNT))] })
                .to_string(),
        },
        RefusedPolicy {
            case: "no Statement",
            policy: json!({ "Version": POLICY_VERSION }).to_string(),
        },
        RefusedPolicy {
            case: "an empty Statement list",
            policy: policy(json!([])),
        },
        RefusedPolicy {
            case: "a Statement that is a string",
            policy: policy(json!("Allow everything")),
        },
        RefusedPolicy {
            case: "a JSON array rather than a policy object",
            policy: json!([allow(required_actions(), json!(OTHER_ACCOUNT))]).to_string(),
        },
        RefusedPolicy {
            case: "text that is not JSON at all",
            policy: "allow-the-other-account-to-use-this-rule-group".to_string(),
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // Two rule groups per row: one already sharing, one sharing nothing, so the
        // refusal is shown to leave BOTH states as they were.
        let sharing_name = format!("refused-sharing-{index}");
        let (_sharing_id, sharing_arn, _sharing_lock) =
            create_rule_group(&waf, "REGIONAL", &sharing_name, 10, json!([])).await;
        waf.ok(
            "PutPermissionPolicy",
            json!({ "ResourceArn": &sharing_arn, "Policy": &baseline }),
        )
        .await;
        let unshared_name = format!("refused-unshared-{index}");
        let (_unshared_id, unshared_arn, _unshared_lock) =
            create_rule_group(&waf, "CLOUDFRONT", &unshared_name, 10, json!([])).await;

        for arn in [&sharing_arn, &unshared_arn] {
            let refusal = waf
                .refused(
                    "PutPermissionPolicy",
                    json!({ "ResourceArn": arn, "Policy": &row.policy }),
                    "WAFInvalidPermissionPolicyException",
                    row.case,
                )
                .await;
            assert_eq!(
                error_message(&refusal),
                INVALID_PERMISSION_POLICY,
                "case {:?}: the refusal must carry the invalid-policy sentence, got {refusal}",
                row.case
            );
            assert!(
                !refusal.to_string().contains(&row.policy),
                "case {:?}: the refusal must disclose nothing of the rejected document, got {refusal}",
                row.case
            );
        }

        assert_stored_policy(&waf, &sharing_arn, &baseline, row.case).await;
        assert_no_stored_policy(&waf, &unshared_arn, row.case).await;
    }
}

/// Deleting the permission policy leaves the rule group sharing nothing, and says so
/// whether or not one was attached: the call names the rule group, "permanently
/// deletes an IAM policy from the specified rule group", and a rule group carrying
/// none is already in that state — so only the rule group itself has to exist.
#[tokio::test]
async fn deleting_the_policy_leaves_the_rule_group_sharing_nothing() {
    let waf = Waf::new();
    let policy = baseline_policy();

    // Both scopes, and both starting states: a rule group that is sharing and one
    // that never was.
    for scope in ["REGIONAL", "CLOUDFRONT"] {
        for attach_first in [true, false] {
            let case = format!("{scope}, policy attached first: {attach_first}");
            let name = format!("deleted-policy-{scope}-{attach_first}");
            let (_id, arn, _lock) = create_rule_group(&waf, scope, &name, 10, json!([])).await;
            if attach_first {
                waf.ok(
                    "PutPermissionPolicy",
                    json!({ "ResourceArn": &arn, "Policy": &policy }),
                )
                .await;
                assert_stored_policy(&waf, &arn, &policy, &case).await;
            }

            // Twice: the second delete finds nothing left to take away and answers
            // the same way, so the end state is what the call states.
            for attempt in 1..=2 {
                let deleted = waf
                    .ok("DeletePermissionPolicy", json!({ "ResourceArn": &arn }))
                    .await;
                assert!(
                    deleted.as_object().is_none_or(|members| members.is_empty()),
                    "case {case:?} attempt {attempt}: DeletePermissionPolicy models an \
                     empty response, got {deleted}"
                );
                assert_no_stored_policy(&waf, &arn, &case).await;
            }

            // The rule group itself survives the delete, and can be shared again.
            waf.ok(
                "PutPermissionPolicy",
                json!({ "ResourceArn": &arn, "Policy": &policy }),
            )
            .await;
            assert_stored_policy(&waf, &arn, &policy, &case).await;
        }
    }
}

/// An address that resolves to no rule group of this account is refused, whatever the
/// policy carries: a value that is not an ARN at all is the parameter failure, and a
/// well-formed ARN naming nothing this service holds — another resource kind, a rule
/// group that was never created, or the same rule group in another Region — is the
/// nonexistent-item failure. Neither stores anything.
#[tokio::test]
async fn an_address_that_names_no_rule_group_is_refused() {
    let waf = Waf::new();
    let (_id, rule_group_arn, _lock) =
        create_rule_group(&waf, "REGIONAL", "addressed-group", 10, json!([])).await;
    let (_acl_id, acl_arn, _acl_lock) =
        create_web_acl(&waf, "REGIONAL", "addressed-acl", json!([])).await;
    let (_ips_id, ips_arn, _ips_lock) = create_ip_set(&waf, "REGIONAL", "addressed-ips").await;
    // The SAME rule group's ARN, carrying another Region: "the rule group must exist
    // in the same Region".
    let other_region_arn = rule_group_arn.replace(TEST_REGION, "eu-west-1");
    let policy = baseline_policy();

    // A value the modelled constraints admit that is not an ARN: `Field`,
    // `Parameter` and `Reason` are what this one exception models, so the rejected
    // value is reported in `Parameter` rather than built into the sentence.
    for operation in [
        "PutPermissionPolicy",
        "GetPermissionPolicy",
        "DeletePermissionPolicy",
    ] {
        let case = format!("{operation} with a value that is not an ARN");
        let refusal = waf
            .refused(
                operation,
                json!({ "ResourceArn": MALFORMED_ARN, "Policy": &policy }),
                "WAFInvalidParameterException",
                &case,
            )
            .await;
        assert_eq!(
            refusal["Parameter"].as_str(),
            Some(MALFORMED_ARN),
            "case {case:?}: the refusal must report the rejected ARN in Parameter, got {refusal}"
        );
        assert_eq!(
            refusal["Field"].as_str(),
            Some("RESOURCE_ARN"),
            "case {case:?}: the refusal must name the field at fault, got {refusal}"
        );
    }

    let nonexistent = [
        ("a rule group ARN no create minted", MISSING_RULE_GROUP_ARN),
        ("a web ACL's ARN", acl_arn.as_str()),
        ("an IP set's ARN", ips_arn.as_str()),
        ("the rule group's ARN in another Region", &other_region_arn),
    ];
    for (case, arn) in nonexistent {
        for operation in ["PutPermissionPolicy", "DeletePermissionPolicy"] {
            let case = format!("{operation}, {case}");
            let refusal = waf
                .refused(
                    operation,
                    json!({ "ResourceArn": arn, "Policy": &policy }),
                    "WAFNonexistentItemException",
                    &case,
                )
                .await;
            assert_eq!(
                error_message(&refusal),
                NONEXISTENT_ITEM,
                "case {case:?}: the refusal must carry the nonexistent-item sentence, \
                 got {refusal}"
            );
        }
        assert_no_stored_policy(&waf, arn, case).await;
    }

    // The rows above are not vacuous: the ARN the create minted is accepted, and the
    // rule group the refused addresses named is still sharing nothing.
    assert_no_stored_policy(&waf, &rule_group_arn, "the created rule group").await;
    waf.ok(
        "PutPermissionPolicy",
        json!({ "ResourceArn": &rule_group_arn, "Policy": &policy }),
    )
    .await;
    assert_stored_policy(&waf, &rule_group_arn, &policy, "the created rule group").await;
    // A policy attached to one rule group is that rule group's own: the web ACL and
    // IP set ARNs still resolve to nothing shareable.
    assert_no_stored_policy(&waf, &other_region_arn, "another Region").await;
}
