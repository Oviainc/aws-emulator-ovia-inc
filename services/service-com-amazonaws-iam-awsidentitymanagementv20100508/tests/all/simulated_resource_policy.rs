// SPDX-License-Identifier: CC0-1.0

//! `SimulatePrincipalPolicy` over a resource-based policy, and the attribution every
//! decision it reports carries.
//!
//! The two rules under test:
//!
//! 1. the policy supplied in `ResourcePolicy` is decided for the SIMULATED CALLER — the
//!    identity `CallerArn` names, defaulting to the one `PolicySourceArn` names — so a
//!    statement grants or denies exactly when its principal element names that identity,
//!    while a statement naming the account hands the decision to that account's own
//!    identity-based policies rather than granting by itself;
//! 2. `MatchedStatements` names the policies that determined the reported decision and
//!    of which kind each is — an identity-based policy in force on the principal, or the
//!    resource-based policy the request supplied — and names none for a decision no
//!    statement stated.
//!
//! Each suite is a TABLE whose rows differ along what the rules generalise over: the
//! form of the statement's principal element (one string, a list, the whole-element
//! wildcard, the negated spelling, the account root), which identity the simulation is
//! of, the effect, the action and the service it belongs to, the resource, where the
//! deciding document came from and its position among the supplied ones, and how many
//! resources one request asks about.
//!
//! The awsQuery harness and the `SimulatePrincipalPolicy` request builder are the ones
//! `tests/all/user_lifecycle.rs` and `tests/all/policy_simulation.rs` own.

use crate::policy_simulation::{principal, simulate, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY};
use crate::user_lifecycle::{element, elements, router};

/// The `Statement$SourcePolicyType` the simulator reports for an identity-based policy
/// in force on the simulated principal, and for the resource-based policy the request
/// supplied: the two kinds of document a simulation decides from.
pub(crate) const IDENTITY_POLICY: &str = "IAM Policy";
pub(crate) const RESOURCE_POLICY: &str = "Resource Policy";

/// The `Statement$SourcePolicyId` the simulator reports for the request's own
/// `ResourcePolicy` member.
pub(crate) const RESOURCE_POLICY_ID: &str = "ResourcePolicy";

/// The `Statement$SourcePolicyId` an INLINE policy is reported under: the embedding it is
/// — the type of the entity it is embedded in, that entity's name and the policy's own name
/// — because an inline policy's name is unique only within the entity that embeds it, so
/// two entities may each embed one of that name and a decision has to be attributable to
/// the one embedding that produced it. A MANAGED policy is reported under its friendly
/// name, which the account already holds it uniquely under.
///
/// Shared by every simulation suite in this crate, so the rule is written exactly once.
pub(crate) fn inline_policy_id(
    attachment_type: &str,
    attachment_name: &str,
    policy_name: &str,
) -> String {
    format!("{attachment_type}_{attachment_name}_{policy_name}")
}

/// [`inline_policy_id`] for the common case: a policy embedded in a USER.
pub(crate) fn user_inline_policy_id(user_name: &str, policy_name: &str) -> String {
    inline_policy_id("user", user_name, policy_name)
}

/// Resources the simulation is about. "You can simulate resources that don't exist in
/// your account" (`SimulatePrincipalPolicy`), so these are never created anywhere; the
/// three services differ so nothing under test can be keyed to one of them.
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/9e1f1b1c-1111-4444-8888-aaaaaaaaaaaa";
const OTHER_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/7c2d2b2c-2222-5555-9999-bbbbbbbbbbbb";
const QUEUE_ARN: &str = "arn:aws:sqs:us-west-2:000000000000:orders";
const OBJECT_ARN: &str = "arn:aws:s3:::reports/june.csv";

/// An inline policy allowing one action everywhere, and one denying it.
const ALLOW_ENCRYPT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"kms:Encrypt","Resource":"*"}]}"#;
const DENY_ENCRYPT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"kms:Encrypt","Resource":"*"}]}"#;

/// The names those two are embedded under, which is how a `MatchedStatements` entry
/// identifies them.
const ALLOW_ENCRYPT_NAME: &str = "allow-encrypt";
const DENY_ENCRYPT_NAME: &str = "deny-encrypt";

/// A key policy whose administrative statement names the account root and whose second
/// statement grants the simulated caller, the shape AWS documents a default key policy
/// in. `{…}` placeholders are filled with the row's own ARNs ([`fill`]).
const KEY_POLICY_FOR_CALLER: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"admin","Effect":"Allow","Principal":{"AWS":"{root}"},"Action":"kms:*","Resource":"*"},{"Sid":"grant","Effect":"Allow","Principal":{"AWS":"{caller}"},"Action":"kms:Encrypt","Resource":"{resource}"}]}"#;

/// The same key policy, granting a principal that is NOT the simulated caller.
const KEY_POLICY_FOR_OTHER: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"admin","Effect":"Allow","Principal":{"AWS":"{root}"},"Action":"kms:*","Resource":"*"},{"Sid":"grant","Effect":"Allow","Principal":{"AWS":"{other}"},"Action":"kms:Encrypt","Resource":"{resource}"}]}"#;

/// The same key policy, granting the principal `PolicySourceArn` names rather than the
/// one `CallerArn` does.
const KEY_POLICY_FOR_POLICY_SOURCE: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"grant","Effect":"Allow","Principal":{"AWS":"{source}"},"Action":"kms:Encrypt","Resource":"{resource}"}]}"#;

/// A key policy naming ONLY the account root: "the default key policy … gives the
/// Amazon Web Services account that owns the KMS key permission to use it", which is a
/// delegation to that account's identity-based policies rather than a grant to a
/// principal.
const KEY_POLICY_FOR_THE_ACCOUNT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"account","Effect":"Allow","Principal":{"AWS":"{root}"},"Action":"kms:*","Resource":"*"}]}"#;

/// A key policy denying the simulated caller, and one denying its whole account.
const KEY_POLICY_DENYING_CALLER: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"refuse","Effect":"Deny","Principal":{"AWS":"{caller}"},"Action":"kms:Encrypt","Resource":"{resource}"}]}"#;
const KEY_POLICY_DENYING_THE_ACCOUNT: &str = r#"{"Version":"2012-10-17","Statement":[{"Sid":"refuse","Effect":"Deny","Principal":{"AWS":"{root}"},"Action":"kms:Encrypt","Resource":"{resource}"}]}"#;

/// A queue policy naming the simulated caller among several principals, and a bucket
/// policy open to every principal: the list and whole-element-wildcard spellings of the
/// principal element.
const QUEUE_POLICY_LISTING_CALLER: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":["{other}","{caller}"]},"Action":"sqs:SendMessage","Resource":"{resource}"}]}"#;
const BUCKET_POLICY_FOR_EVERYONE: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"{resource}"}]}"#;

/// A bucket policy whose statement EXCLUDES another principal, so it covers this one.
const BUCKET_POLICY_EXCLUDING_OTHER: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","NotPrincipal":{"AWS":"{other}"},"Action":"s3:GetObject","Resource":"{resource}"}]}"#;

/// One row of the table.
struct Case {
    case: &'static str,
    /// The inline policies the principal `PolicySourceArn` names carries.
    policies: &'static [(&'static str, &'static str)],
    /// Whether the request names a `CallerArn` of its own: the simulation is then of a
    /// SECOND identity, carrying the policy source's policies.
    caller_is_other: bool,
    /// The document the request supplies in `ResourcePolicy`, if any.
    resource_policy: Option<&'static str>,
    action: &'static str,
    resource: &'static str,
    /// The decision the simulation must report, for the action and for the resource.
    expected: &'static str,
    /// The `(SourcePolicyId, SourcePolicyType)` pairs `MatchedStatements` must carry, in
    /// order: the identity-based policies first, then the resource-based policy.
    expected_matched: &'static [(&'static str, &'static str)],
}

const CASES: &[Case] = &[
    Case {
        case: "a key policy naming the simulated principal grants it",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_CALLER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: ALLOWED,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a key policy naming a different principal grants nothing",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_OTHER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: IMPLICIT_DENY,
        expected_matched: &[],
    },
    Case {
        case: "a queue policy listing the simulated principal among several grants it",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(QUEUE_POLICY_LISTING_CALLER),
        action: "sqs:SendMessage",
        resource: QUEUE_ARN,
        expected: ALLOWED,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a bucket policy open to every principal grants",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(BUCKET_POLICY_FOR_EVERYONE),
        action: "s3:GetObject",
        resource: OBJECT_ARN,
        expected: ALLOWED,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a statement excluding another principal covers the simulated one",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(BUCKET_POLICY_EXCLUDING_OTHER),
        action: "s3:GetObject",
        resource: OBJECT_ARN,
        expected: ALLOWED,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a key policy naming only the account does not grant by itself",
        policies: &[],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_THE_ACCOUNT),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: IMPLICIT_DENY,
        expected_matched: &[],
    },
    Case {
        case: "a key policy naming the account lets the identity policy grant",
        policies: &[(ALLOW_ENCRYPT_NAME, ALLOW_ENCRYPT)],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_THE_ACCOUNT),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: ALLOWED,
        expected_matched: &[(ALLOW_ENCRYPT_NAME, IDENTITY_POLICY)],
    },
    Case {
        case: "an identity policy alone grants and is attributed to itself",
        policies: &[(ALLOW_ENCRYPT_NAME, ALLOW_ENCRYPT)],
        caller_is_other: false,
        resource_policy: None,
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: ALLOWED,
        expected_matched: &[(ALLOW_ENCRYPT_NAME, IDENTITY_POLICY)],
    },
    Case {
        case: "both grant: both policies determined the result",
        policies: &[(ALLOW_ENCRYPT_NAME, ALLOW_ENCRYPT)],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_CALLER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: ALLOWED,
        expected_matched: &[
            (ALLOW_ENCRYPT_NAME, IDENTITY_POLICY),
            (RESOURCE_POLICY_ID, RESOURCE_POLICY),
        ],
    },
    Case {
        case: "a key policy denying the principal is the only entry",
        policies: &[(ALLOW_ENCRYPT_NAME, ALLOW_ENCRYPT)],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_DENYING_CALLER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: EXPLICIT_DENY,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a key policy denying the account denies each of its principals",
        policies: &[(ALLOW_ENCRYPT_NAME, ALLOW_ENCRYPT)],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_DENYING_THE_ACCOUNT),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: EXPLICIT_DENY,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "an identity policy denying the action is reported as the deny",
        policies: &[(DENY_ENCRYPT_NAME, DENY_ENCRYPT)],
        caller_is_other: false,
        resource_policy: Some(KEY_POLICY_FOR_CALLER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: EXPLICIT_DENY,
        expected_matched: &[(DENY_ENCRYPT_NAME, IDENTITY_POLICY)],
    },
    Case {
        case: "nothing states anything about the request",
        policies: &[],
        caller_is_other: false,
        resource_policy: None,
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: IMPLICIT_DENY,
        expected_matched: &[],
    },
    Case {
        case: "the simulation is of the identity CallerArn names",
        policies: &[],
        caller_is_other: true,
        resource_policy: Some(KEY_POLICY_FOR_CALLER),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: ALLOWED,
        expected_matched: &[(RESOURCE_POLICY_ID, RESOURCE_POLICY)],
    },
    Case {
        case: "a key policy naming the policy source does not grant that caller",
        policies: &[],
        caller_is_other: true,
        resource_policy: Some(KEY_POLICY_FOR_POLICY_SOURCE),
        action: "kms:Encrypt",
        resource: KEY_ARN,
        expected: IMPLICIT_DENY,
        expected_matched: &[],
    },
];

/// RULE 1 and RULE 2 over every row of the table.
#[tokio::test]
async fn a_resource_policy_is_decided_for_the_simulated_caller() {
    let router = router();

    for (row, case) in CASES.iter().enumerate() {
        let source = principal(&router, case.case, &format!("source{row}"), case.policies).await;
        let other = principal(&router, case.case, &format!("other{row}"), &[]).await;
        let caller = if case.caller_is_other {
            other.clone()
        } else {
            source.clone()
        };

        let mut extra: Vec<(String, String)> = Vec::new();
        if let Some(document) = case.resource_policy {
            extra.push((
                "ResourcePolicy".to_string(),
                fill(document, &caller, &source, &other, case.resource),
            ));
        }
        if case.caller_is_other {
            extra.push(("CallerArn".to_string(), caller.clone()));
        }
        let (status, body) =
            simulate(&router, &source, &[case.action], &[case.resource], &extra).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            case.expected,
            "case {:?}: expected the action to be decided {:?}, got {body}",
            case.case,
            case.expected
        );
        assert_eq!(
            element(&body, "EvalResourceDecision"),
            case.expected,
            "case {:?}: expected the resource to be decided {:?}, got {body}",
            case.case,
            case.expected
        );

        let expected: Vec<(String, String)> = case
            .expected_matched
            .iter()
            .map(|(id, kind)| {
                // Every identity-based entry in this table is an inline policy of the
                // principal the row created, so it is named by that embedding.
                let id = if *kind == IDENTITY_POLICY {
                    user_inline_policy_id(&format!("source{row}"), id)
                } else {
                    (*id).to_string()
                };
                (id, (*kind).to_string())
            })
            .collect();
        let (top, per_resource) = parts(&body);
        assert_eq!(
            attribution(top),
            expected,
            "case {:?}: expected the action's result to name {expected:?}, got {body}",
            case.case
        );
        assert_eq!(
            attribution(per_resource),
            expected,
            "case {:?}: expected the resource's result to name {expected:?}, got {body}",
            case.case
        );
    }
}

/// Which of the documents a request supplies grants the action, counted from one: the
/// rows differ in how many are supplied and which of them decides.
const SUPPLIED: &[&[bool]] = &[
    &[true],
    &[true, false, false],
    &[false, true, false],
    &[false, false, true],
    &[true, false, true],
];

/// RULE 2 for a document the REQUEST supplied: it is identified by the member it
/// arrived in and its position in it.
#[tokio::test]
async fn a_supplied_policy_is_attributed_to_its_position_in_the_input_list() {
    let router = router();
    let unrelated = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"kms:Decrypt","Resource":"*"}]}"#;

    for (row, granting) in SUPPLIED.iter().enumerate() {
        let case = format!("{} supplied, granting {granting:?}", granting.len());
        let arn = principal(&router, &case, &format!("supplied{row}"), &[]).await;
        let extra: Vec<(String, String)> = granting
            .iter()
            .enumerate()
            .map(|(at, grants)| {
                (
                    format!("PolicyInputList.member.{}", at + 1),
                    if *grants { ALLOW_ENCRYPT } else { unrelated }.to_string(),
                )
            })
            .collect();
        let (status, body) = simulate(&router, &arn, &["kms:Encrypt"], &[KEY_ARN], &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            ALLOWED,
            "case {case:?}: expected a supplied Allow to grant, got {body}"
        );
        let expected: Vec<(String, String)> = granting
            .iter()
            .enumerate()
            .filter(|(_, grants)| **grants)
            .map(|(at, _)| {
                (
                    format!("PolicyInputList.{}", at + 1),
                    IDENTITY_POLICY.to_string(),
                )
            })
            .collect();
        let (top, _) = parts(&body);
        assert_eq!(
            attribution(top),
            expected,
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// RULE 2 across resources: "in the top-level result, this field contains the union of
/// matched statements across all requested resources", while a resource nothing stated
/// anything about names none.
#[tokio::test]
async fn the_actions_result_reports_the_union_across_every_resource() {
    let router = router();
    let arn = principal(&router, "two resources", "union", &[]).await;
    let policy = fill(KEY_POLICY_FOR_CALLER, &arn, &arn, &arn, KEY_ARN);

    let (status, body) = simulate(
        &router,
        &arn,
        &["kms:Encrypt"],
        &[KEY_ARN, OTHER_KEY_ARN],
        &[("ResourcePolicy".to_string(), policy)],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "EvalResourceDecision"),
        vec![ALLOWED.to_string(), IMPLICIT_DENY.to_string()],
        "expected only the resource the key policy names to be allowed: {body}"
    );
    let (top, per_resource) = parts(&body);
    assert_eq!(
        attribution(top),
        vec![(
            RESOURCE_POLICY_ID.to_string(),
            RESOURCE_POLICY.to_string()
        )],
        "expected the action's result to carry the union of the two resources: {body}"
    );
    assert_eq!(
        attribution(per_resource),
        vec![(
            RESOURCE_POLICY_ID.to_string(),
            RESOURCE_POLICY.to_string()
        )],
        "expected only the allowed resource's own result to name a policy: {body}"
    );
}

/// The row's own ARNs, put into a policy template's placeholders.
fn fill(template: &str, caller: &str, source: &str, other: &str, resource: &str) -> String {
    template
        .replace("{caller}", caller)
        .replace("{source}", source)
        .replace("{other}", other)
        .replace("{root}", &account_root(caller))
        .replace("{resource}", resource)
}

/// The account-root ARN of the account the principal `arn` names belongs to: the five
/// leading segments of its own ARN, so the partition and account are the ones that ARN
/// carries.
fn account_root(arn: &str) -> String {
    let envelope: Vec<&str> = arn.split(':').take(5).collect();
    format!("{}:root", envelope.join(":"))
}

/// The response split into the evaluation result's OWN members and its per-resource
/// ones: the serializer emits `ResourceSpecificResults` after the members of the result
/// that carries it, so the two attributions are read apart.
pub(crate) fn parts(body: &str) -> (&str, &str) {
    body.split_once("<ResourceSpecificResults>")
        .unwrap_or((body, ""))
}

/// The `(SourcePolicyId, SourcePolicyType)` pairs one part of the response reports, in
/// document order.
pub(crate) fn attribution(part: &str) -> Vec<(String, String)> {
    let ids = elements(part, "SourcePolicyId");
    let kinds = elements(part, "SourcePolicyType");
    assert_eq!(
        ids.len(),
        kinds.len(),
        "every matched statement names both the policy and its type: {part}"
    );
    ids.into_iter().zip(kinds).collect()
}
