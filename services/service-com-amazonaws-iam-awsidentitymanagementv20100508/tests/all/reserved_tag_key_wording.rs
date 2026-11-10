// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the refusal IAM answers a reserved tag key with.
//!
//! RULE under test: the `aws:` tag-key prefix is reserved ("you cannot create tag keys that
//! begin with `aws:` because this prefix is reserved for Amazon Web Services use", "Tagging
//! IAM resources"), and a request naming one is refused with the modelled
//! `InvalidInputException` — HTTP 400, awsQuery code `InvalidInput` — carrying AWS's ONE
//! fixed sentence for that refusal class. The sentence is a constant: it is the same text
//! whichever operation refused, whichever key of the request was reserved, wherever in the
//! list it sat and however it was capitalised, and it carries nothing of the request, so a
//! client that surfaces or matches AWS's reserved-prefix wording recognises it.
//!
//! The suite is a TABLE whose rows differ along what the rule generalises over: which
//! operation carries the tags (a `Tag*` call that writes onto a held resource and a
//! `Create*` call that carries them with the create), which kind of resource it addresses,
//! the reserved key itself and its capitalisation, and the position of the reserved key
//! among valid ones.
//!
//! The rows that assert nothing is STORED by a refused request live in the per-resource
//! suites (`tests/all/instance_profile_tags.rs`, `tests/all/user_tags.rs`,
//! `tests/all/role_tags.rs`, `tests/all/policy_tags.rs`); the axis here is the sentence.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, error_code, router};

/// The sentence AWS answers a reserved tag key with, written out here rather than read
/// from the implementation — the wording is what this suite is about, so reading it off the
/// constant the service carries would assert nothing.
const RESERVED_PREFIX_SENTENCE: &str = "Tag keys beginning with aws: are reserved for system use.";

/// A trust policy `CreateRole` accepts: the account's own principal, as the other role
/// suites in this crate create a fixture role with.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        crate::user_lifecycle::account_id()
    )
}

/// A permission document a managed policy can carry.
const SOME_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;

/// The reserved keys the rows name, spanning the prefix alone, a key AWS itself writes,
/// and the prefix in three capitalisations — the prefix is reserved however it is spelled.
const RESERVED_KEYS: &[&str] = &[
    "aws:",
    "aws:reserved-marker",
    "aws:cloudformation:stack-name",
    "AWS:Owner",
    "Aws:Owner",
];

/// Which operation a row drives, and the resource it addresses.
#[derive(Clone, Copy, Debug)]
enum Target {
    /// `TagUser` onto a held user.
    User,
    /// `TagRole` onto a held role.
    Role,
    /// `TagInstanceProfile` onto a held instance profile.
    InstanceProfile,
    /// `TagPolicy` onto a held managed policy.
    Policy,
    /// `CreateUser` carrying the tags with the create.
    CreateUser,
    /// `CreateRole` carrying them with the create.
    CreateRole,
    /// `CreateInstanceProfile` carrying them with the create.
    CreateInstanceProfile,
    /// `CreatePolicy` carrying them with the create.
    CreatePolicy,
}

impl Target {
    /// Every target, so one table spans the `Tag*` and the `Create*` halves of the rule.
    const ALL: &'static [Self] = &[
        Self::User,
        Self::Role,
        Self::InstanceProfile,
        Self::Policy,
        Self::CreateUser,
        Self::CreateRole,
        Self::CreateInstanceProfile,
        Self::CreatePolicy,
    ];

    /// The awsQuery `Action` this target's refusal comes from.
    fn action(self) -> &'static str {
        match self {
            Self::User => "TagUser",
            Self::Role => "TagRole",
            Self::InstanceProfile => "TagInstanceProfile",
            Self::Policy => "TagPolicy",
            Self::CreateUser => "CreateUser",
            Self::CreateRole => "CreateRole",
            Self::CreateInstanceProfile => "CreateInstanceProfile",
            Self::CreatePolicy => "CreatePolicy",
        }
    }
}

/// Create whatever the row's target needs to exist before the tagging call, and answer the
/// parameters that address it — the held resource for a `Tag*` row, or the names and
/// documents a `Create*` row carries.
async fn addressed(
    router: &MultiServiceRouter,
    target: Target,
    name: &str,
) -> Vec<(String, String)> {
    match target {
        Target::User => {
            expect_ok(router, "CreateUser", &[("UserName", name)]).await;
            vec![("UserName".to_string(), name.to_string())]
        }
        Target::Role => {
            expect_ok(
                router,
                "CreateRole",
                &[("RoleName", name), ("AssumeRolePolicyDocument", &trust_policy())],
            )
            .await;
            vec![("RoleName".to_string(), name.to_string())]
        }
        Target::InstanceProfile => {
            expect_ok(router, "CreateInstanceProfile", &[("InstanceProfileName", name)]).await;
            vec![("InstanceProfileName".to_string(), name.to_string())]
        }
        Target::Policy => {
            let (status, created) = call(
                router,
                "CreatePolicy",
                &[("PolicyName", name), ("PolicyDocument", SOME_DOCUMENT)],
            )
            .await;
            assert_eq!(
                status, 200,
                "the fixture CreatePolicy must succeed, got {status}: {created}"
            );
            vec![("PolicyArn".to_string(), element(&created, "Arn"))]
        }
        Target::CreateUser => vec![("UserName".to_string(), name.to_string())],
        Target::CreateRole => vec![
            ("RoleName".to_string(), name.to_string()),
            ("AssumeRolePolicyDocument".to_string(), trust_policy()),
        ],
        Target::CreateInstanceProfile => {
            vec![("InstanceProfileName".to_string(), name.to_string())]
        }
        Target::CreatePolicy => vec![
            ("PolicyName".to_string(), name.to_string()),
            ("PolicyDocument".to_string(), SOME_DOCUMENT.to_string()),
        ],
    }
}

/// The `Tags.member.N.{Key,Value}` parameters for `tags`, as the query protocol flattens a
/// tag list.
fn tag_parameters(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(at, (key, value))| {
            [
                (format!("Tags.member.{}.Key", at + 1), (*key).to_string()),
                (format!("Tags.member.{}.Value", at + 1), (*value).to_string()),
            ]
        })
        .collect()
}

/// RULE over every (operation, reserved key) pair: the refusal is HTTP 400 `InvalidInput`
/// carrying AWS's one fixed sentence, with nothing of the request in it.
#[tokio::test]
async fn every_reserved_prefix_refusal_carries_awss_fixed_sentence() {
    let router = router();

    for (row, target) in Target::ALL.iter().enumerate() {
        for (column, reserved) in RESERVED_KEYS.iter().enumerate() {
            let case = format!("{} refusing {reserved:?}", target.action());
            let name = format!("reserved-{row}-{column}");
            let mut params = addressed(&router, *target, &name).await;
            // The reserved key sits SECOND, behind a valid one, so the sentence cannot be
            // keyed to the first tag of the request either.
            params.extend(tag_parameters(&[("owner", "platform"), (reserved, "x")]));
            let borrowed: Vec<(&str, &str)> = params
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str()))
                .collect();

            let (status, body) = call(&router, target.action(), &borrowed).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for a reserved tag key, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, RESERVED_PREFIX_SENTENCE,
                "case {case:?}: expected AWS's fixed sentence \
                 {RESERVED_PREFIX_SENTENCE:?}, got {message:?}"
            );
        }
    }
}

/// RULE's control: a tag list of the same shape whose keys are NOT reserved is served, so
/// the refusal above is the prefix and not the request's shape.
#[tokio::test]
async fn a_tag_list_outside_the_reserved_prefix_is_served() {
    let router = router();

    for (row, target) in Target::ALL.iter().enumerate() {
        let case = format!("{} serving unreserved keys", target.action());
        let name = format!("allowed-{row}");
        let mut params = addressed(&router, *target, &name).await;
        // Keys that merely MENTION the prefix without beginning with it: the rule is about
        // the beginning of the key.
        params.extend(tag_parameters(&[
            ("owner", "platform"),
            ("not-aws:reserved", "x"),
            ("awsuite", "y"),
        ]));
        let borrowed: Vec<(&str, &str)> = params
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();

        let (status, body) = call(&router, target.action(), &borrowed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the unreserved keys to be served, got {status}: {body}"
        );
    }
}

/// Make one call that must succeed for a row to be set up at all.
async fn expect_ok(router: &MultiServiceRouter, action: &str, params: &[(&str, &str)]) {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "the fixture {action} must succeed, got {status}: {body}"
    );
}
