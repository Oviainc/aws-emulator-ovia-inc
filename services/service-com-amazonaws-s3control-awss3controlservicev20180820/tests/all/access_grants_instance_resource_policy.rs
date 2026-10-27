// SPDX-License-Identifier: CC0-1.0

//! The resource policy of an S3 Access Grants instance, driven end to end over
//! HTTP.
//!
//! The rule under test: the resource policy is a subresource of ONE Access
//! Grants instance — the one the request's account owns in the Region it was
//! signed for — so a put stores the document it was given on that instance and
//! reports it with the organization it named and the instant it was stored, a
//! read answers the stored document byte for byte, a further put replaces it,
//! and a delete leaves the instance with none, which a read then refuses. An
//! account with no instance in that Region has no resource to carry a policy.
//!
//! Every row below differs along something that rule generalises over: the form
//! and size of the policy document (the shortest policy document this surface
//! takes, one statement, several statements with conditions, a document with
//! characters the XML envelope has to escape), whether the put
//! names an organization and the length of the one it names, and the account
//! and Region that own the instance.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder, the XML escaping and the readers are the sibling suites',
//! and so is the call that creates an instance — reused rather than copied.

use std::collections::BTreeSet;

use crate::access_grants_data_plane::create_instance;
use crate::access_point_lifecycle::{
    call_in_region, xml_escape, Emulator, Reply, ACCOUNT, OTHER_ACCOUNT, REGION, XMLNS,
};

/// A second signing Region, used to pin that a policy belongs to the instance of
/// one account in one Region.
const OTHER_REGION: &str = "eu-central-1";

/// The fixed sentence S3 Access Grants answers when the instance carries no
/// resource policy.
const NO_POLICY_MESSAGE: &str = "Access Grants Instance Resource Policy does not exist";

/// Store `policy` as the resource policy of the account's instance in `region`,
/// naming `organization` when there is one.
async fn put_policy(
    emulator: &Emulator,
    account_id: &str,
    region: &str,
    policy: &str,
    organization: Option<&str>,
) -> Reply {
    let mut body = format!(
        "<PutAccessGrantsInstanceResourcePolicyRequest xmlns=\"{XMLNS}\"><Policy>{}</Policy>",
        xml_escape(policy)
    );
    if let Some(organization) = organization {
        body.push_str(&format!(
            "<Organization>{}</Organization>",
            xml_escape(organization)
        ));
    }
    body.push_str("</PutAccessGrantsInstanceResourcePolicyRequest>");
    call_in_region(
        &emulator.router,
        region,
        "PUT",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        &body,
    )
    .await
}

/// Read the resource policy of the account's instance in `region`.
async fn get_policy(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        "",
    )
    .await
}

/// Remove the resource policy of the account's instance in `region`.
async fn delete_policy(emulator: &Emulator, account_id: &str, region: &str) -> Reply {
    call_in_region(
        &emulator.router,
        region,
        "DELETE",
        "/v20180820/accessgrantsinstance/resourcepolicy",
        account_id,
        "",
    )
    .await
}

/// One policy case: the instance it is put on, the documents successive puts
/// store, and the organization each names.
struct PolicyCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    /// The (document, organization) pairs successive puts store, in order. More
    /// than one row has several, because a put replaces the policy.
    puts: Vec<(String, Option<&'static str>)>,
}

/// A resource policy document granting `action` to `principal`, the shape an IAM
/// resource policy takes.
fn policy_document(principal: &str, action: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\"Principal\":\
         {{\"AWS\":\"{principal}\"}},\"Action\":[\"{action}\"],\"Resource\":\"*\"}}]}}"
    )
}

/// The policy cases every assertion below is run over.
fn policy_cases() -> Vec<PolicyCase> {
    vec![
        PolicyCase {
            case: "one statement, no organization",
            account_id: ACCOUNT,
            region: REGION,
            puts: vec![(
                policy_document("arn:aws:iam::123456789012:root", "s3:GetDataAccess"),
                None,
            )],
        },
        PolicyCase {
            case: "replaced twice, the second put naming an organization",
            account_id: ACCOUNT,
            region: OTHER_REGION,
            puts: vec![
                (
                    policy_document("arn:aws:iam::123456789012:root", "s3:GetDataAccess"),
                    None,
                ),
                (
                    policy_document("arn:aws:iam::210987654321:root", "s3:ListAccessGrants"),
                    // `Organization`'s pattern is `^o-[a-z0-9]{10,32}$`: this is
                    // the shortest id it admits.
                    Some("o-abcde12345"),
                ),
                (
                    // Several statements, each with a condition.
                    "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                     \"Principal\":{\"AWS\":\"*\"},\"Action\":[\"s3:GetDataAccess\"],\
                     \"Resource\":\"*\",\"Condition\":{\"StringEquals\":\
                     {\"aws:PrincipalOrgID\":\"o-abcde12345\"}}},{\"Effect\":\"Deny\",\
                     \"Principal\":{\"AWS\":\"*\"},\"Action\":[\"s3:DeleteAccessGrant\"],\
                     \"Resource\":\"*\"}]}"
                        .to_string(),
                    // The longest id `Organization`'s pattern admits: `o-` and
                    // 32 characters.
                    Some("o-abcdefghij0123456789abcdefghij01"),
                ),
            ],
        },
        PolicyCase {
            case: "the shortest policy document this surface takes",
            account_id: OTHER_ACCOUNT,
            region: REGION,
            // `PolicyDocument` is constrained to `len 1..350000`; this is a
            // policy document close to that floor — an empty statement list is
            // the shortest one an IAM resource policy has.
            puts: vec![(
                "{\"Version\":\"2012-10-17\",\"Statement\":[]}".to_string(),
                Some("o-abcde12345"),
            )],
        },
        PolicyCase {
            case: "a document carrying characters the XML envelope escapes",
            account_id: ACCOUNT,
            region: "ap-southeast-2",
            puts: vec![(
                policy_document(
                    "arn:aws:iam::123456789012:role/reader&writer",
                    "s3:GetDataAccess",
                ),
                None,
            )],
        },
    ]
}

/// Stand an emulator up with an Access Grants instance for every (account,
/// Region) pair the cases use.
async fn emulator_with_instances(cases: &[PolicyCase]) -> Emulator {
    let emulator = Emulator::start();
    let mut created = BTreeSet::new();
    for case in cases {
        if created.insert((case.account_id, case.region)) {
            let instance = create_instance(&emulator, case.account_id, case.region).await;
            assert_eq!(
                instance.status, 200,
                "case {:?}: expected CreateAccessGrantsInstance to answer 200, got {} with body {}",
                case.case, instance.status, instance.body
            );
        }
    }
    emulator
}

/// Assert `reply` is the refusal an instance carrying no resource policy gets.
fn assert_no_policy(case: &str, call: &str, reply: &Reply) {
    assert_eq!(
        (reply.status, reply.code()),
        (
            404,
            "AccessGrantsInstanceResourcePolicyNotExists".to_string()
        ),
        "case {case:?}: expected {call} to be refused for an instance with no policy, got {} \
         with body {}",
        reply.status,
        reply.body
    );
    assert!(
        reply.body.contains(NO_POLICY_MESSAGE),
        "case {case:?}: expected the no-policy message, got body {}",
        reply.body
    );
}

#[tokio::test]
async fn a_put_policy_is_what_every_later_read_reports_until_it_is_replaced() {
    let cases = policy_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        // An instance created without a policy carries none.
        assert_no_policy(
            case.case,
            "GetAccessGrantsInstanceResourcePolicy before any put",
            &get_policy(&emulator, case.account_id, case.region).await,
        );

        for (policy, organization) in &case.puts {
            let put = put_policy(
                &emulator,
                case.account_id,
                case.region,
                policy,
                *organization,
            )
            .await;
            assert_eq!(
                put.status, 200,
                "case {:?}: expected the put to answer 200, got {} with body {}",
                case.case, put.status, put.body
            );
            assert_eq!(
                put.text("Policy").as_deref(),
                Some(policy.as_str()),
                "case {:?}: expected the put to report the document it stored, got body {}",
                case.case,
                put.body
            );
            assert_eq!(
                put.text("Organization").as_deref(),
                *organization,
                "case {:?}: expected the put to report the organization it was given, got body {}",
                case.case,
                put.body
            );
            assert!(
                put.text("CreatedAt").is_some(),
                "case {:?}: expected the put to report when the policy was stored, got body {}",
                case.case,
                put.body
            );

            // The read reports the stored document, and reports it twice over:
            // a read changes nothing.
            let read = get_policy(&emulator, case.account_id, case.region).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected the read to answer 200, got {} with body {}",
                case.case, read.status, read.body
            );
            assert_eq!(
                read.text("Policy").as_deref(),
                Some(policy.as_str()),
                "case {:?}: expected the read to report the stored document, got body {}",
                case.case,
                read.body
            );
            assert_eq!(
                read.text("Organization").as_deref(),
                *organization,
                "case {:?}: expected the read to report the stored organization, got body {}",
                case.case,
                read.body
            );
            assert_eq!(
                read.text("CreatedAt"),
                put.text("CreatedAt"),
                "case {:?}: expected the read to report the instant the put did, got body {}",
                case.case,
                read.body
            );
            let read_again = get_policy(&emulator, case.account_id, case.region).await;
            assert_eq!(
                read_again.body, read.body,
                "case {:?}: expected a second read to report the same policy, got body {}",
                case.case, read_again.body
            );
        }
    }

    // Every instance carries its OWN policy: the last put of each case is what
    // that instance reports, and no other instance's put changed it.
    for case in &cases {
        let (policy, organization) = case.puts.last().expect("a case puts a policy");
        let read = get_policy(&emulator, case.account_id, case.region).await;
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(policy.as_str()),
            "case {:?}: expected the instance to carry its own last put, got body {}",
            case.case,
            read.body
        );
        assert_eq!(
            read.text("Organization").as_deref(),
            *organization,
            "case {:?}: expected the instance to carry its own organization, got body {}",
            case.case,
            read.body
        );
    }
}

#[tokio::test]
async fn a_deleted_policy_is_gone_and_can_be_put_again() {
    let cases = policy_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        let (policy, organization) = &case.puts[0];
        assert_eq!(
            put_policy(
                &emulator,
                case.account_id,
                case.region,
                policy,
                *organization
            )
            .await
            .status,
            200,
            "case {:?}: expected the put to succeed",
            case.case
        );

        let deleted = delete_policy(&emulator, case.account_id, case.region).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: expected the delete to answer 200, got {} with body {}",
            case.case, deleted.status, deleted.body
        );
        assert_no_policy(
            case.case,
            "GetAccessGrantsInstanceResourcePolicy after the delete",
            &get_policy(&emulator, case.account_id, case.region).await,
        );

        // Deleting the policy of an instance that carries none leaves it with
        // none: the delete asks for a state, not for a record to exist.
        let again = delete_policy(&emulator, case.account_id, case.region).await;
        assert_eq!(
            again.status, 200,
            "case {:?}: expected a second delete to answer 200, got {} with body {}",
            case.case, again.status, again.body
        );

        // The instance itself survived the delete, so a policy can be put on it
        // again and is read back.
        let repeat = put_policy(
            &emulator,
            case.account_id,
            case.region,
            policy,
            *organization,
        )
        .await;
        assert_eq!(
            repeat.status, 200,
            "case {:?}: expected a put after the delete to answer 200, got {} with body {}",
            case.case, repeat.status, repeat.body
        );
        assert_eq!(
            get_policy(&emulator, case.account_id, case.region)
                .await
                .text("Policy")
                .as_deref(),
            Some(policy.as_str()),
            "case {:?}: expected the re-put policy to read back",
            case.case
        );
    }
}

#[tokio::test]
async fn a_document_that_is_not_a_policy_is_refused_and_stores_nothing() {
    let cases = policy_cases();
    let emulator = emulator_with_instances(&cases).await;
    let case = &cases[0];
    let (policy, organization) = &case.puts[0];
    assert_eq!(
        put_policy(
            &emulator,
            case.account_id,
            case.region,
            policy,
            *organization
        )
        .await
        .status,
        200,
        "expected the first put to succeed"
    );

    // Each row is a document that is not JSON at all, so it is not a policy
    // this service stores.
    for malformed in [
        "not a policy",
        "{\"Version\":\"2012-10-17\"",
        "<Policy>denied</Policy>",
    ] {
        let refused = put_policy(&emulator, case.account_id, case.region, malformed, None).await;
        assert_eq!(
            (refused.status, refused.code()),
            (400, "MalformedPolicy".to_string()),
            "case {malformed:?}: expected a document that is not JSON to be refused, got {} with \
             body {}",
            refused.status,
            refused.body
        );
        // Nothing was stored: the instance still carries the policy it had.
        let read = get_policy(&emulator, case.account_id, case.region).await;
        assert_eq!(
            read.text("Policy").as_deref(),
            Some(policy.as_str()),
            "case {malformed:?}: expected the refused put to leave the stored policy alone, got \
             body {}",
            read.body
        );
    }
}

#[tokio::test]
async fn an_account_with_no_instance_in_that_region_has_no_policy_to_carry() {
    let cases = policy_cases();
    let emulator = emulator_with_instances(&cases).await;
    let (policy, _) = &cases[0].puts[0];

    // Each row addresses an instance that was never created: another account in
    // a Region where it has none, and the owning account in a Region where it
    // has none.
    let rows: Vec<(&str, &str, &str)> = vec![
        ("another account, no instance", OTHER_ACCOUNT, OTHER_REGION),
        ("the owning account, no instance", ACCOUNT, "eu-west-1"),
    ];

    for (label, account_id, region) in rows {
        for (call, reply) in [
            (
                "PutAccessGrantsInstanceResourcePolicy",
                put_policy(&emulator, account_id, region, policy, None).await,
            ),
            // The resource is resolved before the document is looked at, so a
            // document this service would not store is still answered with the
            // missing instance rather than with the policy refusal.
            (
                "PutAccessGrantsInstanceResourcePolicy with a malformed document",
                put_policy(&emulator, account_id, region, "not a policy", None).await,
            ),
            (
                "GetAccessGrantsInstanceResourcePolicy",
                get_policy(&emulator, account_id, region).await,
            ),
            (
                "DeleteAccessGrantsInstanceResourcePolicy",
                delete_policy(&emulator, account_id, region).await,
            ),
        ] {
            assert_eq!(
                (reply.status, reply.code()),
                (404, "AccessGrantsInstanceNotExistsError".to_string()),
                "case {label:?}: expected {call} to be refused with no instance there, got {} \
                 with body {}",
                reply.status,
                reply.body
            );
        }
    }
}
