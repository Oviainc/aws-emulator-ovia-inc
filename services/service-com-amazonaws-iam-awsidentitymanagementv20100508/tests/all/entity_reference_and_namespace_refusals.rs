// SPDX-License-Identifier: CC0-1.0

//! How IAM refuses a value it cannot read — for a member that must name ONE IAM identity,
//! for a member that must BE a managed policy's ARN, and for a service namespace — end to
//! end over the awsQuery wire.
//!
//! Three rules are under test, each as a TABLE whose rows differ along what the rule
//! generalises over rather than along one recorded request:
//!
//! 1. A member whose documentation says it names an IAM user, group or role is refused,
//!    when the value cannot be read as one, with the sentence the OPERATION words that
//!    refusal in: the policy simulator's members state that the request provided no usable
//!    ARN and name the value, while `ListPoliciesGrantingServiceAccess` states that the
//!    entity ARN is not supported. Either way the refusal is the modelled
//!    `InvalidInputException` (HTTP 400, awsQuery code `InvalidInput`), and the sentence is
//!    the same whatever is wrong with the value.
//!
//! 2. A member that must name ONE thing is refused when the value stands for a SET of
//!    names (it carries a wildcard): the account a simulation's resources belong to, and
//!    the resources a resource-based policy the request supplies is attached to. The
//!    refusal states that the value defines neither type nor name. The wildcard resource
//!    set stays served where no resource-based policy is supplied, since that is the value
//!    `ResourceArns` defaults to.
//!
//! 3. A member that must BE a managed policy's ARN, handed a value that is not an ARN at
//!    all, is refused HTTP 400 `ValidationError` — and WHICH sentence it carries goes by
//!    what the operation has to decide about the ARN: the operations that must decide
//!    whether the policy is ATTACHABLE (the ones modelling `PolicyNotAttachableException`:
//!    the three attaches and the two permissions-boundary puts) read the value themselves
//!    and name it in the refusal, while every other operation hands it to IAM's generic ARN
//!    parser, whose sentence states the reading it managed between two fixed halves and so
//!    quotes nothing of the value.
//!
//! 4. A service namespace a request asks about has to NAME a service: the namespaces are
//!    AWS's own published prefixes, so a value that passes `serviceNamespaceType`'s pattern
//!    but is published for no service is refused `InvalidInput` naming every rejected
//!    namespace, rather than answered with an empty list of policies.
//!
//! Each suite also reads the resource back through an implemented read afterwards, so a
//! refusal is shown to leave the account exactly as it was.
//!
//! The awsQuery harness (router, call, element, elements, error_code) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares; the simulation
//! request spelling is `tests/all/policy_simulation.rs`'s.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;

use crate::policy_simulation::{reported, simulate};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A trust policy every role here needs to exist at all.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"glue.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The document the fixture policy carries, and the one a `CreatePolicyVersion` row sends.
const DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The action every simulation row asks about.
const ACTION: &str = "s3:GetObject";

/// What a simulation of [`DOCUMENT`] must decide for [`ACTION`], which is what the suites
/// read back to show a refused call decided nothing and changed nothing.
const ALLOWED_DECISION: &str = "allowed";

/// One form parameter, owned.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The parameters as the harness takes them.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect()
}

/// AWS's sentence for a value supplied for a member that must BE an ARN when nothing of it
/// reads as one: fixed text followed by the value the request supplied.
fn arn_not_provided(value: &str) -> String {
    format!("Invalid ARN provided in the request: {value}")
}

/// AWS's sentence for a value that stands for a SET of names where ONE entity belongs.
fn no_single_entity(value: &str) -> String {
    format!("Invalid Entity Arn: {value} does not clearly define entity type and name.")
}

/// AWS's sentence for a well-formed ARN naming a resource the member cannot act on.
fn unsupported_entity(value: &str) -> String {
    format!("Invalid Entity Arn: {value} is not supported.")
}

/// The two fixed halves of AWS's sentence for a value its ARN parser cannot read, written
/// out here rather than read from the implementation's constant, and the empty reading an
/// unparseable value leaves between them — the suite pins the SENTENCE, so a reworded
/// constant has to fail here rather than agree with itself.
const UNPARSEABLE_ARN: &str = "Invalid ARN:  Could not be parsed!";

/// AWS's sentence for a value the attach-side reading cannot use as a policy ARN: fixed
/// text around the value the request supplied.
fn arn_not_valid(value: &str) -> String {
    format!("ARN {value} is not valid.")
}

/// Values that are no ARN AT ALL, each at least 20 characters so `arnType`'s own length
/// constraint (20..2048) is satisfied and the handler is what refuses them. The rows break
/// the `arn:` frame a different way each: a bare friendly name, the service and resource
/// with no frame, a frame spelled in upper case, too few fields, text before the first
/// field, and punctuation alone.
const NOT_AN_ARN: &[(&str, &str)] = &[
    ("a bare friendly name", "AWSLakeFormationDataAdmin"),
    (
        "the service and resource with no frame",
        "iam::aws:policy/AmazonS3ReadOnlyAccess",
    ),
    (
        "a frame spelled in upper case",
        "ARN:aws:iam::aws:policy/ReadOnlyAccess",
    ),
    ("an ARN missing its resource field", "arn:aws:iam::123456789012"),
    (
        "text before the first field",
        " arn:aws:iam::aws:policy/ReadOnlyAccess",
    ),
    ("punctuation alone", "::::::::::::::::::::"),
];

/// Create a user, answering the ARN the account holds it under.
async fn create_user(router: &MultiServiceRouter, user_name: &str) -> String {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {user_name:?}: CreateUser must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a group, answering the ARN the account holds it under.
async fn create_group(router: &MultiServiceRouter, group_name: &str) -> String {
    let (status, body) = call(router, "CreateGroup", &[("GroupName", group_name)]).await;
    assert_eq!(
        status, 200,
        "case {group_name:?}: CreateGroup must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Create a role, answering the ARN the account holds it under.
async fn create_role(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// Embed `DOCUMENT` in the user as an inline policy, so a simulation of its policies has
/// something to decide from.
async fn put_user_policy(router: &MultiServiceRouter, user_name: &str) {
    let (status, body) = call(
        router,
        "PutUserPolicy",
        &[
            ("UserName", user_name),
            ("PolicyName", "reads"),
            ("PolicyDocument", DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "PutUserPolicy must succeed, got {status}: {body}"
    );
}

/// Create a customer managed policy, answering its ARN.
async fn create_policy(router: &MultiServiceRouter, policy_name: &str) -> String {
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[
            ("PolicyName", policy_name),
            ("PolicyDocument", DOCUMENT),
            ("Path", "/eng/"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {policy_name:?}: CreatePolicy must succeed, got {status}: {body}"
    );
    element(&body, "Arn")
}

/// One `SimulatePrincipalPolicy` of the principal `policy_source_arn`, with `extra`
/// carrying the row's member.
async fn simulate_principal(
    router: &MultiServiceRouter,
    policy_source_arn: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    simulate(router, policy_source_arn, &[ACTION], &[], extra).await
}

/// One `SimulateCustomPolicy` deciding [`DOCUMENT`], with `extra` carrying the row's
/// member.
async fn simulate_custom(
    router: &MultiServiceRouter,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![
        param("PolicyInputList.member.1", DOCUMENT),
        param("ActionNames.member.1", ACTION),
    ];
    params.extend(extra.iter().cloned());
    call(router, "SimulateCustomPolicy", &borrowed(&params)).await
}

/// What a simulation of the fixture user's policies decides, read back to show a refused
/// call left the policies in force exactly as they were.
async fn decided(router: &MultiServiceRouter, user_arn: &str) -> Vec<(String, String)> {
    let (status, body) = simulate_principal(router, user_arn, &[]).await;
    assert_eq!(
        status, 200,
        "the control simulation must succeed, got {status}: {body}"
    );
    reported(&body)
}

/// One member that must name ONE IAM identity, as this suite drives it: the case name, the
/// sentence its operation words the refusal in, and the extra parameters the member needs
/// beside the value.
struct IdentityMember {
    case: &'static str,
    sentence: fn(&str) -> String,
    extra: fn(&str) -> Vec<(String, String)>,
}

/// RULE 1: a member that must name ONE IAM user, group or role is refused 400
/// `InvalidInput` with the sentence the OPERATION words that refusal in — the simulator's
/// members name the value the request supplied, this listing states the entity ARN is not
/// supported — whatever is wrong with the value.
#[tokio::test]
async fn a_member_naming_one_identity_is_refused_by_the_sentence_of_its_operation() {
    let router = router();
    let account = account_id();
    let user_arn = create_user(&router, "EntityReader").await;
    put_user_policy(&router, "EntityReader").await;
    let before = decided(&router, &user_arn).await;
    assert_eq!(
        before,
        vec![(ACTION.to_string(), ALLOWED_DECISION.to_string())],
        "the fixture must decide {ACTION} as {ALLOWED_DECISION}, got {before:?}"
    );

    // Rows: the values a member that must name one identity cannot be read from, spanning
    // the ways a value misses — nothing of it reads as an ARN ([`NOT_AN_ARN`]), or it is a
    // well-formed ARN of a resource that is no user, group or role (a managed policy in
    // either namespace, an instance profile, another service's resource). A value standing
    // for a SET of names carries its own sentence, asserted by the sibling suite below.
    // Every value is at least 20 characters, since `arnType` (`PolicySourceArn`, `Arn`)
    // declares that floor.
    let mut values: Vec<String> = Vec::new();
    for (case, value) in NOT_AN_ARN {
        assert!(
            value.len() >= 20,
            "case {case:?}: a row must satisfy arnType's 20-character minimum, got {value:?}"
        );
        values.push((*value).to_string());
    }
    values.extend([
        format!("arn:aws:iam::{account}:policy/Readers"),
        "arn:aws:iam::aws:policy/ReadOnlyAccess".to_string(),
        format!("arn:aws:iam::{account}:instance-profile/Build"),
        "arn:aws:s3:::reports-bucket-of-this-account".to_string(),
        "arn:aws:sts::210987654321:assumed-role/deployer/session".to_string(),
    ]);

    // The members that must name one identity, each as the request that carries the row's
    // value, beside the sentence its operation words the refusal in: the simulator's own
    // (the principal it reads policies for, and the simulated caller of either
    // simulation), and this listing's.
    let user_arn_for_caller = user_arn.clone();
    let members = [
        IdentityMember {
            case: "SimulatePrincipalPolicy.PolicySourceArn",
            sentence: arn_not_provided,
            extra: |_value: &str| Vec::new(),
        },
        IdentityMember {
            case: "SimulatePrincipalPolicy.CallerArn",
            sentence: arn_not_provided,
            extra: |value: &str| vec![param("CallerArn", value)],
        },
        IdentityMember {
            case: "GetContextKeysForPrincipalPolicy.PolicySourceArn",
            sentence: arn_not_provided,
            extra: |_value: &str| Vec::new(),
        },
        IdentityMember {
            case: "SimulateCustomPolicy.CallerArn",
            sentence: arn_not_provided,
            extra: |value: &str| vec![param("CallerArn", value)],
        },
        IdentityMember {
            case: "ListPoliciesGrantingServiceAccess.Arn",
            sentence: unsupported_entity,
            extra: |_value: &str| vec![param("ServiceNamespaces.member.1", "s3")],
        },
    ];

    for IdentityMember {
        case: member,
        sentence,
        extra: build,
    } in &members
    {
        for value in &values {
            let expected = sentence(value);
            let extra = build(value);
            let (status, body) = match *member {
                "SimulatePrincipalPolicy.PolicySourceArn" => {
                    simulate_principal(&router, value, &extra).await
                }
                "SimulatePrincipalPolicy.CallerArn" => {
                    simulate_principal(&router, &user_arn_for_caller, &extra).await
                }
                "GetContextKeysForPrincipalPolicy.PolicySourceArn" => {
                    let mut params = vec![param("PolicySourceArn", value)];
                    params.extend(extra.clone());
                    call(
                        &router,
                        "GetContextKeysForPrincipalPolicy",
                        &borrowed(&params),
                    )
                    .await
                }
                "SimulateCustomPolicy.CallerArn" => simulate_custom(&router, &extra).await,
                _ => {
                    let mut params = vec![param("Arn", value)];
                    params.extend(extra.clone());
                    call(
                        &router,
                        "ListPoliciesGrantingServiceAccess",
                        &borrowed(&params),
                    )
                    .await
                }
            };
            let case = format!("{member} = {value:?}");
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case:?}: expected InvalidInput, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    // Every call above was refused, so the policies in force on the principal are the ones
    // the fixture put there and a simulation still decides alike.
    let after = decided(&router, &user_arn).await;
    assert_eq!(
        after, before,
        "a refused call must leave the decision unchanged: expected {before:?}, got {after:?}"
    );
}

/// RULE 2: a member that must name ONE thing — an identity, the account a simulation's
/// resources belong to, or a resource the request's own resource-based policy is attached
/// to — is refused 400 `InvalidInput` when the value stands for a SET of names, with the
/// sentence stating that it defines neither type nor name; while the forms that do name one
/// thing are served.
#[tokio::test]
async fn a_value_standing_for_a_set_of_names_is_refused_where_one_thing_belongs() {
    let router = router();
    let account = account_id();
    let user_arn = create_user(&router, "WildcardReader").await;
    put_user_policy(&router, "WildcardReader").await;

    // A resource-based policy for the rows about the resources one is attached to: "each
    // resource in the simulation is treated as if it had this policy attached"
    // (`ResourcePolicy`).
    let resource_policy = format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{user_arn}"}},"Action":"{ACTION}","Resource":"*"}}]}}"#
    );

    // Rows: the member carrying a value that stands for a set of names, and that value.
    // `ResourceOwner` and `CallerArn` target `ResourceNameType` (length 1..2048), so the
    // bare wildcard — one character — reaches the handler rather than a modelled
    // constraint, while `PolicySourceArn` and `Arn` (`arnType`, length 20..2048) carry a
    // wildcard inside an otherwise well-formed ARN. The rows span both wildcards the policy
    // language matches a set with and every position one can sit in: the entity's name, its
    // account, and the whole resource.
    let mut rows: Vec<(String, String, (u16, String))> = Vec::new();
    for value in ["*", "?", "1234567890??"] {
        rows.push((
            format!("SimulatePrincipalPolicy.CallerArn = {value:?}"),
            value.to_string(),
            simulate_principal(&router, &user_arn, &[param("CallerArn", value)]).await,
        ));
        rows.push((
            format!("SimulateCustomPolicy.CallerArn = {value:?}"),
            value.to_string(),
            simulate_custom(&router, &[param("CallerArn", value)]).await,
        ));
        rows.push((
            format!("SimulatePrincipalPolicy.ResourceOwner = {value:?}"),
            value.to_string(),
            simulate_principal(&router, &user_arn, &[param("ResourceOwner", value)]).await,
        ));
        rows.push((
            format!("SimulateCustomPolicy.ResourceOwner = {value:?}"),
            value.to_string(),
            simulate_custom(&router, &[param("ResourceOwner", value)]).await,
        ));
    }
    for value in [
        "arn:aws:iam::123456789012:user/*".to_string(),
        "arn:aws:iam::123456789012:role/Deployer?".to_string(),
        "arn:aws:iam::*:role/DeployerRoleHere".to_string(),
        format!("arn:aws:iam::{account}:*"),
    ] {
        rows.push((
            format!("SimulatePrincipalPolicy.PolicySourceArn = {value:?}"),
            value.clone(),
            simulate_principal(&router, &value, &[]).await,
        ));
        rows.push((
            format!("GetContextKeysForPrincipalPolicy.PolicySourceArn = {value:?}"),
            value.clone(),
            call(
                &router,
                "GetContextKeysForPrincipalPolicy",
                &borrowed(&[param("PolicySourceArn", &value)]),
            )
            .await,
        ));
        rows.push((
            format!("ListPoliciesGrantingServiceAccess.Arn = {value:?}"),
            value.clone(),
            call(
                &router,
                "ListPoliciesGrantingServiceAccess",
                &borrowed(&[
                    param("Arn", &value),
                    param("ServiceNamespaces.member.1", "s3"),
                ]),
            )
            .await,
        ));
    }
    // The resources a supplied resource-based policy is attached to: the wildcard resource
    // set itself, and ARNs carrying a wildcard where the resource's name belongs. An entry
    // that is no ARN at all is refused by the member's own format rule before this one, so
    // the rows here are values that ARE resource names apart from standing for a set.
    for value in [
        "*",
        "arn:aws:s3:::reports-bucket/*",
        "arn:aws:sqs:us-west-2:210987654321:orders-?",
    ] {
        let with_policy_and_resource = vec![
            param("ResourcePolicy", &resource_policy),
            param("CallerArn", &user_arn),
            param("ResourceArns.member.1", value),
        ];
        rows.push((
            format!("SimulatePrincipalPolicy.ResourceArns = {value:?} with a resource policy"),
            value.to_string(),
            simulate_principal(&router, &user_arn, &with_policy_and_resource).await,
        ));
        rows.push((
            format!("SimulateCustomPolicy.ResourceArns = {value:?} with a resource policy"),
            value.to_string(),
            simulate_custom(&router, &with_policy_and_resource).await,
        ));
    }

    for (case, value, (status, body)) in rows {
        let expected = no_single_entity(&value);
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A request that supplies a resource-based policy and names NO resource is the same
    // case: "if this parameter is not provided, then the value defaults to * (all
    // resources)" (`ResourceArns`), so the policy has no one resource to be attached to
    // and the refusal names that default.
    let expected = no_single_entity("*");
    let (status, body) = simulate_principal(
        &router,
        &user_arn,
        &[
            param("ResourcePolicy", &resource_policy),
            param("CallerArn", &user_arn),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a resource policy with no resource named must be refused, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "Message"),
        expected,
        "expected {expected:?}, got {:?}",
        element(&body, "Message")
    );

    // The other side of the rule: a value that names ONE thing is served — the forms
    // `ResourceOwner` documents (a bare account id and an account's root ARN, for this
    // account and for another one) …
    for owner in [
        account.to_string(),
        "210987654321".to_string(),
        format!("arn:aws:iam::{account}:root"),
        "arn:aws:iam::210987654321:root".to_string(),
    ] {
        let (status, body) =
            simulate_principal(&router, &user_arn, &[param("ResourceOwner", &owner)]).await;
        assert_eq!(
            status, 200,
            "case {owner:?}: a ResourceOwner naming one account must be served, got \
             {status}: {body}"
        );
    }

    // … a resource-based policy attached to resources that each name one …
    for resource in [
        "arn:aws:s3:::reports-bucket/june.csv",
        "arn:aws:sqs:us-west-2:210987654321:orders",
    ] {
        let (status, body) = simulate_principal(
            &router,
            &user_arn,
            &[
                param("ResourcePolicy", &resource_policy),
                param("CallerArn", &user_arn),
                param("ResourceArns.member.1", resource),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {resource:?}: a named resource must be served, got {status}: {body}"
        );
    }

    // … and the wildcard resource set itself, where the request supplies no resource-based
    // policy to attach: it is then the value the member defaults to and names the
    // resources the simulation decides about.
    for resources in [Vec::new(), vec![param("ResourceArns.member.1", "*")]] {
        let (status, body) = simulate_principal(&router, &user_arn, &resources).await;
        assert_eq!(
            status, 200,
            "case {resources:?}: the wildcard resource set the member defaults to must be \
             served without a resource policy, got {status}: {body}"
        );
    }
}

/// One operation taking a managed policy's ARN: the request it is called with, and whether
/// it is one of the operations that must decide the policy's ATTACHABILITY.
struct PolicyArnMember {
    /// The operation name on the wire.
    action: &'static str,
    /// Every parameter besides the ARN member, which the row supplies.
    others: Vec<(String, String)>,
    /// The member the ARN arrives in.
    arn_member: &'static str,
    /// Whether the operation models `PolicyNotAttachableException` — the operations that
    /// decide whether the ARN names an attachable policy, and so read it themselves.
    decides_attachability: bool,
}

/// RULE 3: a value that is not an ARN at all is refused 400 `ValidationError` by every
/// member taking a managed policy's ARN, and the sentence goes by what the operation has to
/// decide about the ARN — the attach-side operations name the value, every other one states
/// IAM's generic parse failure, which quotes nothing of it.
#[tokio::test]
async fn an_unparseable_policy_arn_is_refused_by_the_sentence_of_its_operation() {
    let router = router();
    create_user(&router, "ArnFormUser").await;
    create_group(&router, "ArnFormGroup").await;
    create_role(&router, "ArnFormRole").await;
    let held = create_policy(&router, "ArnFormPolicy").await;

    let members = vec![
        PolicyArnMember {
            action: "GetPolicy",
            others: Vec::new(),
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "GetPolicyVersion",
            others: vec![param("VersionId", "v1")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "ListPolicyVersions",
            others: Vec::new(),
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "ListPolicyTags",
            others: Vec::new(),
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "ListEntitiesForPolicy",
            others: Vec::new(),
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "CreatePolicyVersion",
            others: vec![param("PolicyDocument", DOCUMENT)],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "SetDefaultPolicyVersion",
            others: vec![param("VersionId", "v1")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "DeletePolicyVersion",
            others: vec![param("VersionId", "v1")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "DeletePolicy",
            others: Vec::new(),
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "TagPolicy",
            others: vec![
                param("Tags.member.1.Key", "team"),
                param("Tags.member.1.Value", "storage"),
            ],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "UntagPolicy",
            others: vec![param("TagKeys.member.1", "team")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "DetachUserPolicy",
            others: vec![param("UserName", "ArnFormUser")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "DetachGroupPolicy",
            others: vec![param("GroupName", "ArnFormGroup")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "DetachRolePolicy",
            others: vec![param("RoleName", "ArnFormRole")],
            arn_member: "PolicyArn",
            decides_attachability: false,
        },
        PolicyArnMember {
            action: "AttachUserPolicy",
            others: vec![param("UserName", "ArnFormUser")],
            arn_member: "PolicyArn",
            decides_attachability: true,
        },
        PolicyArnMember {
            action: "AttachGroupPolicy",
            others: vec![param("GroupName", "ArnFormGroup")],
            arn_member: "PolicyArn",
            decides_attachability: true,
        },
        PolicyArnMember {
            action: "AttachRolePolicy",
            others: vec![param("RoleName", "ArnFormRole")],
            arn_member: "PolicyArn",
            decides_attachability: true,
        },
        PolicyArnMember {
            action: "PutUserPermissionsBoundary",
            others: vec![param("UserName", "ArnFormUser")],
            arn_member: "PermissionsBoundary",
            decides_attachability: true,
        },
        PolicyArnMember {
            action: "PutRolePermissionsBoundary",
            others: vec![param("RoleName", "ArnFormRole")],
            arn_member: "PermissionsBoundary",
            decides_attachability: true,
        },
    ];

    for member in &members {
        for (how, value) in NOT_AN_ARN {
            let mut params = vec![param(member.arn_member, value)];
            params.extend(member.others.iter().cloned());
            let (status, body) = call(&router, member.action, &borrowed(&params)).await;
            let case = format!("{} with {how} ({value:?})", member.action);
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationError",
                "case {case:?}: expected ValidationError, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            if member.decides_attachability {
                assert_eq!(
                    message,
                    arn_not_valid(value),
                    "case {case:?}: an attach-side refusal names the value, got {message:?}"
                );
            } else {
                assert_eq!(
                    message, UNPARSEABLE_ARN,
                    "case {case:?}: expected {UNPARSEABLE_ARN:?}, got {message:?}"
                );
                assert!(
                    !message.contains(value.trim()),
                    "case {case:?}: the parse failure quotes nothing of the value, got \
                     {message:?}"
                );
                assert!(
                    message.contains("  "),
                    "case {case:?}: the empty reading is rendered between the template's \
                     two spaces, got {message:?}"
                );
            }
        }
    }

    // Every call above was refused, so the policy the fixture created is still held and
    // still carries its first version as the operative one.
    let (status, body) = call(&router, "GetPolicy", &[("PolicyArn", held.as_str())]).await;
    assert_eq!(
        status, 200,
        "the held policy must survive every refused call, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "DefaultVersionId"),
        "v1",
        "a refused call must move no version, got {body}"
    );
    assert_eq!(
        element(&body, "Arn"),
        held,
        "the held policy must read back under its own ARN, got {body}"
    );
}

/// RULE 4: the namespaces a request asks about have to name services AWS publishes — every
/// one that does not is named in the refusal, and a request whose namespaces all name one
/// is served.
#[tokio::test]
async fn a_service_namespace_no_service_is_published_under_is_refused() {
    let router = router();
    let user_arn = create_user(&router, "NamespaceReader").await;
    put_user_policy(&router, "NamespaceReader").await;

    // SOURCE for every published prefix below: this crate's catalog of the services a
    // policy can name an action of (`state::REPORTED_SERVICES`), which is the Service
    // Authorization Reference's own list of prefixes. The rows take entries the issue's
    // recording did not cite — the catalog's first and last entry and two from its middle
    // — so the rule is not pinned to one namespace.
    let published: Vec<&str> = state::REPORTED_SERVICES
        .iter()
        .map(|service| service.service_namespace)
        .collect();
    let first = published.first().copied().expect("the catalog carries rows");
    let last = published.last().copied().expect("the catalog carries rows");
    for namespace in [first, last, "lakeformation", "sts"] {
        assert!(
            published.contains(&namespace),
            "case {namespace:?}: the row must name a published prefix"
        );
        let params = vec![
            param("Arn", &user_arn),
            param("ServiceNamespaces.member.1", namespace),
        ];
        let (status, body) = call(
            &router,
            "ListPoliciesGrantingServiceAccess",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {namespace:?}: a published prefix must be served, got {status}: {body}"
        );
        assert_eq!(
            elements(&body, "ServiceNamespace"),
            vec![namespace.to_string()],
            "case {namespace:?}: the entry must answer the namespace asked about, got {body}"
        );
    }

    // A prefix is published in lower case and this service reads a namespace against the
    // catalog the way the sibling read does — case folded — so another case of a published
    // prefix names the same service.
    let params = vec![
        param("Arn", &user_arn),
        param("ServiceNamespaces.member.1", "S3"),
    ];
    let (status, body) = call(
        &router,
        "ListPoliciesGrantingServiceAccess",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "a published prefix in another case must be served, got {status}: {body}"
    );

    // Rows: the namespaces one request asks about, and the ones the refusal must name — in
    // the order the request named them. They differ along how the value misses the catalog
    // (a vendor prefix in front of the service, a service's full name, a word no service
    // is published under), how many of them miss it, and where in the list they sit.
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("a vendor prefix in front of the service", vec!["amazon-s3"], vec!["amazon-s3"]),
        (
            "a service's full name rather than its prefix",
            vec!["simple-storage-service"],
            vec!["simple-storage-service"],
        ),
        (
            "a word no service is published under",
            vec!["not_a_service"],
            vec!["not_a_service"],
        ),
        (
            "one published prefix and one that is not",
            vec!["s3", "amazon-s3"],
            vec!["amazon-s3"],
        ),
        (
            "an unpublished prefix before a published one",
            vec!["amazon-ec2", "ec2"],
            vec!["amazon-ec2"],
        ),
        (
            "two that are not published, in the request's order",
            vec!["amazon-s3", "amazon-dynamodb"],
            vec!["amazon-s3", "amazon-dynamodb"],
        ),
        (
            "every namespace of the request unpublished",
            vec!["aws-lambda", "aws-sqs", "aws-sns"],
            vec!["aws-lambda", "aws-sqs", "aws-sns"],
        ),
    ];

    for (case, asked, rejected) in rows {
        for namespace in &asked {
            assert!(
                !published.contains(namespace) || asked.len() > rejected.len(),
                "case {case:?}: {namespace:?} must be unpublished unless the row mixes in a \
                 published one"
            );
        }
        let mut params = vec![param("Arn", &user_arn)];
        for (position, namespace) in asked.iter().enumerate() {
            params.push(param(
                &format!("ServiceNamespaces.member.{}", position + 1),
                namespace,
            ));
        }
        let (status, body) = call(
            &router,
            "ListPoliciesGrantingServiceAccess",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let expected = format!("Invalid service namespace: [{}]", rejected.join(", "));
        let message = element(&body, "Message");
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert!(
            elements(&body, "ServiceNamespace").is_empty(),
            "case {case:?}: a refused request reports on no namespace, got {body}"
        );
    }

    // The refusals reported nothing and changed nothing: the identity's policies are still
    // the ones the fixture put in force, reported for a published prefix.
    let params = vec![
        param("Arn", &user_arn),
        param("ServiceNamespaces.member.1", "s3"),
    ];
    let (status, body) = call(
        &router,
        "ListPoliciesGrantingServiceAccess",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "the control request must be served, got {status}: {body}"
    );
    assert_eq!(
        elements(&body, "PolicyName"),
        vec!["reads".to_string()],
        "the inline policy granting s3 must still be reported, got {body}"
    );
}
