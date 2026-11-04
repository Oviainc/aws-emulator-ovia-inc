// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the organization the emulated account BELONGS to.
//!
//! The rule: the account this emulator presents is a member of an organization, and is
//! that organization's management account, from the moment the Organizations control
//! plane is mounted — so every read of the organization answers it instead of refusing
//! with "not a member of an organization", and a create is the refusal an account that is
//! already in one gets. A client that resolves its organization before calling an
//! organization-scoped API (of this service or of any other) therefore learns the
//! organization and its management account id.
//!
//! Asserted over sets rather than over one call: every read of the implemented slice,
//! each read taken twice (a read must not change what the next caller sees), over
//! several independent boots of the emulator, and the create refused over the model's
//! whole `OrganizationFeatureSet` catalogue plus the request that omits it.
//!
//! The members are checked against their modelled patterns
//! (`smithy_query(query="shape", shape="Organization")`): `Id` `^o-[a-z0-9]{10,32}$`,
//! `Arn` `^arn:aws:organizations::\d{12}:organization\/o-[a-z0-9]{10,32}$`,
//! `MasterAccountId` `^\d{12}$`, `MasterAccountArn`
//! `^arn:aws:organizations::\d{12}:account\/o-[a-z0-9]{10,32}\/\d{12}$` and
//! `MasterAccountEmail` `^[^\s@]+@[^\s@]+\.[^\s@]+$`.
//!
//! The wire harness is `organization_hierarchy_lifecycle`'s, so these assertions are
//! about the same deserialize → handle → serialize walk a client drives.

use serde_json::{json, Value};
use service_com_amazonaws_organizations_awsorganizationsv20161128::ops::types::ORGANIZATION_FEATURE_SET_VALUES;
use service_com_amazonaws_organizations_awsorganizationsv20161128::state::{
    DEFAULT_FEATURE_SET, POLICY_TYPE_SERVICE_CONTROL_POLICY, POLICY_TYPE_STATUS_ENABLED, ROOT_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::organization_hierarchy_lifecycle::{account_id, call, matches_id, seeded_router, Answer};

/// How many independent mounts of the control plane each rule is asserted over, so a
/// value that happened to be right once cannot carry the assertion.
const BOOTS: usize = 3;

/// The `Organization` a booted emulator reports, asserted to be there at all.
async fn described_organization(router: &MultiServiceRouter, case: &str) -> Value {
    let answer = call(router, "DescribeOrganization", &json!({})).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        !answer.is("AWSOrganizationsNotInUseException"),
        "case {case}: the account must be a member of an organization, got {:?}",
        answer.error
    );
    answer.body["Organization"].clone()
}

/// Assert `organization` is the organization of THIS account, member by member against
/// the modelled patterns.
fn assert_organization_of_this_account(case: &str, organization: &Value) {
    let id = organization["Id"].as_str().unwrap_or_default();
    assert!(
        matches_id(id, "o-", 10, 32),
        "case {case}: expected an Id matching ^o-[a-z0-9]{{10,32}}$, got {organization:?}"
    );
    let account = account_id();
    assert_eq!(
        organization["Arn"].as_str(),
        Some(format!("arn:aws:organizations::{account}:organization/{id}").as_str()),
        "case {case}: expected the organization's modelled ARN, got {organization:?}"
    );
    assert_eq!(
        organization["MasterAccountId"].as_str(),
        Some(account),
        "case {case}: the emulated account is the management account, got {organization:?}"
    );
    assert_eq!(
        organization["MasterAccountArn"].as_str(),
        Some(format!("arn:aws:organizations::{account}:account/{id}/{account}").as_str()),
        "case {case}: expected the management account's modelled ARN, got {organization:?}"
    );
    let email = organization["MasterAccountEmail"].as_str().unwrap_or_default();
    assert!(
        email.split('@').count() == 2
            && !email.starts_with('@')
            && email.rsplit('@').next().is_some_and(|domain| {
                domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
            })
            && !email.contains(char::is_whitespace),
        "case {case}: expected an address matching Email's ^[^\\s@]+@[^\\s@]+\\.[^\\s@]+$, \
         got {email:?}"
    );
    assert_eq!(
        organization["FeatureSet"].as_str(),
        Some(DEFAULT_FEATURE_SET),
        "case {case}: expected the feature set a create that names none asks for, \
         got {organization:?}"
    );
    assert!(
        ORGANIZATION_FEATURE_SET_VALUES
            .contains(&organization["FeatureSet"].as_str().unwrap_or_default()),
        "case {case}: the feature set must be a value the model's enum declares, \
         got {organization:?}"
    );
}

/// Every boot of the emulator reports the organization this account belongs to, and
/// reports it identically on a second read.
#[tokio::test]
async fn every_booted_emulator_reports_the_organization_the_account_belongs_to() {
    for boot in 1..=BOOTS {
        let router = seeded_router();
        let case = format!("DescribeOrganization on boot {boot}");
        let first = described_organization(&router, &case).await;
        assert_organization_of_this_account(&case, &first);

        // A read does not change what the next caller sees.
        let second = described_organization(&router, &format!("{case}, read 2")).await;
        assert_eq!(
            first, second,
            "case {case}: two reads must answer identically, got {first:?} then {second:?}"
        );

        // The hierarchy the organization record names is really there: exactly one root,
        // carrying the organization's id in its ARN and the policy types the feature set
        // enables.
        let roots = call(&router, "ListRoots", &json!({})).await;
        assert_eq!(
            roots.status, 200,
            "case ListRoots on boot {boot}: expected 200, got {} with {:?}",
            roots.status, roots.body
        );
        let listed = roots.body["Roots"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "case ListRoots on boot {boot}: expected exactly one root, got {listed:?}"
        );
        let organization_id = first["Id"].as_str().unwrap_or_default();
        let root_id = listed[0]["Id"].as_str().unwrap_or_default();
        assert!(
            matches_id(root_id, "r-", 4, 32),
            "case ListRoots on boot {boot}: expected an Id matching ^r-[0-9a-z]{{4,32}}$, \
             got {listed:?}"
        );
        assert_eq!(
            listed[0]["Arn"].as_str(),
            Some(
                format!(
                    "arn:aws:organizations::{}:root/{organization_id}/{root_id}",
                    account_id()
                )
                .as_str()
            ),
            "case ListRoots on boot {boot}: the root's ARN must name this organization, \
             got {listed:?}"
        );
        assert_eq!(
            listed[0]["Name"].as_str(),
            Some(ROOT_NAME),
            "case ListRoots on boot {boot}: expected the root's name, got {listed:?}"
        );
        assert_eq!(
            listed[0]["PolicyTypes"],
            json!([{ "Type": POLICY_TYPE_SERVICE_CONTROL_POLICY, "Status": POLICY_TYPE_STATUS_ENABLED }]),
            "case ListRoots on boot {boot}: an all-features organization enables service \
             control policies in its root, got {listed:?}"
        );
        assert_eq!(
            first["AvailablePolicyTypes"], listed[0]["PolicyTypes"],
            "case boot {boot}: the organization's deprecated policy-type list must report \
             what the root reports, got {first:?} and {listed:?}"
        );
    }
}

/// No read of the implemented slice tells this account it is in no organization, and the
/// hierarchy under the seeded root is usable without any create: an OU hangs under it and
/// is read back and listed there.
#[tokio::test]
async fn the_hierarchy_of_the_organization_is_usable_without_a_create() {
    let router = seeded_router();
    let organization = described_organization(&router, "DescribeOrganization").await;
    let organization_id = organization["Id"].as_str().unwrap_or_default().to_owned();
    let roots = call(&router, "ListRoots", &json!({})).await;
    let root_id = roots.body["Roots"][0]["Id"]
        .as_str()
        .unwrap_or_default()
        .to_owned();

    let created = call(
        &router,
        "CreateOrganizationalUnit",
        &json!({ "ParentId": root_id, "Name": "Sandbox" }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case CreateOrganizationalUnit under the seeded root: expected 200, got {} with {:?}",
        created.status, created.body
    );
    let ou = created.body["OrganizationalUnit"].clone();
    let ou_id = ou["Id"].as_str().unwrap_or_default().to_owned();
    let root_suffix = root_id.strip_prefix("r-").unwrap_or(&root_id);
    assert!(
        matches_id(&ou_id, &format!("ou-{root_suffix}-"), 8, 32),
        "case CreateOrganizationalUnit under the seeded root: expected an Id in this root, \
         got {ou:?}"
    );
    assert_eq!(
        ou["Path"].as_str(),
        Some(format!("{organization_id}/{root_id}/{ou_id}/").as_str()),
        "case CreateOrganizationalUnit under the seeded root: expected the path through \
         this organization, got {ou:?}"
    );

    // Each read of the slice answers from that state, and none of them refuses.
    let reads: Vec<(&str, Value)> = vec![
        ("DescribeOrganization", json!({})),
        ("ListRoots", json!({})),
        (
            "DescribeOrganizationalUnit",
            json!({ "OrganizationalUnitId": ou_id }),
        ),
        (
            "ListOrganizationalUnitsForParent",
            json!({ "ParentId": root_id }),
        ),
    ];
    for (operation, body) in reads {
        for round in 1..=2 {
            let answer: Answer = call(&router, operation, &body).await;
            assert_eq!(
                answer.status, 200,
                "case {operation}, read {round}: expected 200, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                !answer.is("AWSOrganizationsNotInUseException"),
                "case {operation}, read {round}: this account is a member of an \
                 organization, got {:?}",
                answer.error
            );
        }
    }
    let listed = call(
        &router,
        "ListOrganizationalUnitsForParent",
        &json!({ "ParentId": root_id }),
    )
    .await;
    assert_eq!(
        listed.body["OrganizationalUnits"][0]["Id"].as_str(),
        Some(ou_id.as_str()),
        "case ListOrganizationalUnitsForParent(seeded root): expected the created OU, \
         got {:?}",
        listed.body
    );
}

/// An account that is already in an organization cannot create one, whatever feature set
/// the create asks for — and the refusal leaves the organization exactly as it was.
#[tokio::test]
async fn a_create_is_refused_because_this_account_is_already_in_an_organization() {
    // The model's own `OrganizationFeatureSet` catalogue, plus `None` for a request that
    // omits the optional member.
    let mut cases: Vec<Option<&str>> = vec![None];
    cases.extend(ORGANIZATION_FEATURE_SET_VALUES.iter().copied().map(Some));

    for requested in cases {
        let router = seeded_router();
        let case = format!("CreateOrganization(FeatureSet={requested:?})");
        let before = described_organization(&router, &case).await;

        let answer = call(
            &router,
            "CreateOrganization",
            &match requested {
                Some(value) => json!({ "FeatureSet": value }),
                None => json!({}),
            },
        )
        .await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.is("AlreadyInOrganizationException"),
            "case {case}: expected AlreadyInOrganizationException, got {:?}",
            answer.error
        );

        let after = described_organization(&router, &format!("{case}, after the refusal")).await;
        assert_eq!(
            before, after,
            "case {case}: the refusal must change nothing, got {before:?} then {after:?}"
        );
        assert_organization_of_this_account(&case, &after);
    }
}
