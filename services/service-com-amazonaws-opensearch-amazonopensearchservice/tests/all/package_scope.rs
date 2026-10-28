// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdatePackageScope`: the set of other users a package is
//! shared with, which `DescribePackages` reports as its `AllowListedUserList`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`] and the unknown-id constant from
//! [`super::package_associations`]; this suite adds no second copy of either.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * an account that may not vend packages is refused every scope update, over every
//!   `PackageScopeOperationEnum` value and whatever users it names, with the stored
//!   scope left empty;
//! * for an account that may, each operation applies the users named to the stored set
//!   as a union (`ADD`), a replacement (`OVERRIDE`) or a difference (`REMOVE`), over a
//!   table of eleven steps that differ in operation, list length, repeats within the
//!   request, users already in the scope and users absent from it — each step read
//!   back through `DescribePackages`, so the answer and a later read cannot disagree;
//! * a scope is a SET: no operation leaves a user in it twice, and a step that asks
//!   for the scope the package already has leaves the record, `LastUpdatedAt`
//!   included, exactly as it was;
//! * only a package whose vending options enable vending can be shared, over every
//!   `PackageType` value and all three forms of the vending container;
//! * a package's scope is bounded: the bound is accepted, one past it is refused
//!   whether asked for in one call or by adding to a scope already at it, and a
//!   request that withdraws access is accepted at the bound;
//! * an id that names no package of the account is refused as a bad input value, over
//!   every form the `PackageID` pattern admits.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    PACKAGE_SCOPE_OPERATION_ENUM_VALUES, PACKAGE_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    PACKAGE_NOT_FOUND, PACKAGE_SCOPE_QUOTA, PACKAGE_TYPE_ZIP_PLUGIN,
};

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, SETTLE_WAIT};
use super::package_associations::UNKNOWN_PACKAGE_ID;

/// Three accounts a package can be shared with. `PackageUser` is `@length(min: 6,
/// max: 12)` with pattern `^[0-9]{12}$|^GLOBAL$`, so a named user is a 12-digit
/// account id.
const ACCOUNT_A: &str = "111122223333";
const ACCOUNT_B: &str = "222233334444";
const ACCOUNT_C: &str = "333344445555";

/// A fourth account, never granted anything, used to ask for a grant that is not
/// there.
const ACCOUNT_D: &str = "444455556666";

/// `PackageUser`'s other alternative: every user rather than one named account.
const GLOBAL: &str = "GLOBAL";

/// The refusal an account that may not vend packages gets.
const NOT_AUTHORIZED: &str = "You are not authorized to perform this operation.";

/// `UpdatePackageScope`: `POST /2021-01-01/packages/updateScope`.
async fn update_scope(
    emulator: &Emulator,
    package_id: &str,
    operation: &str,
    users: &[&str],
) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/updateScope"),
            Some(json!({
                "PackageID": package_id,
                "Operation": operation,
                "PackageUserList": users,
            })),
        )
        .await
}

/// `CreatePackage` for `name`, answering with the `PackageID` it minted.
///
/// `vending` is the `PackageVendingOptions` to create it with, absent where `None`.
async fn created_package(
    emulator: &Emulator,
    name: &str,
    package_type: &str,
    vending: Option<bool>,
) -> String {
    let case = format!("CreatePackage({name:?}, {package_type:?}, vending {vending:?})");
    let mut body = json!({
        "PackageName": name,
        "PackageType": package_type,
        "PackageSource": {
            "S3BucketName": "my-package-bucket",
            "S3Key": format!("packages/{name}"),
        },
    });
    if let Some(enabled) = vending {
        body["PackageVendingOptions"] = json!({ "VendingEnabled": enabled });
    }
    let answer = emulator
        .call("POST", &format!("{PREFIX}/packages"), Some(body))
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetails"]["PackageID"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a PackageID, got {:?}", answer.body))
        .to_string()
}

/// The `PackageDetails` `DescribePackages` reports for `package_id` — the read path a
/// scope update is proved through.
async fn reported_details(emulator: &Emulator, package_id: &str, case: &str) -> Value {
    let answer = emulator
        .call(
            "POST",
            &format!("{PREFIX}/packages/describe"),
            Some(json!({
                "Filters": [{ "Name": "PackageID", "Value": [package_id] }],
            })),
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribePackages to answer 200, got {} with {:?}",
        answer.status, answer.body
    );
    let reported = answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageDetailsList array, got {:?}",
                answer.body
            )
        });
    assert_eq!(
        reported.len(),
        1,
        "case {case}: expected exactly the one package {package_id}, got {reported:?}"
    );
    reported[0].clone()
}

/// The scope `DescribePackages` reports for `package_id`, as a list of users — empty
/// where the package is shared with nobody, which is how the read path reports a
/// package whose `AllowListedUserList` is absent.
async fn reported_scope(emulator: &Emulator, package_id: &str, case: &str) -> Vec<String> {
    let details = reported_details(emulator, package_id, case).await;
    match details.get("AllowListedUserList") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(users)) => users
            .iter()
            .map(|user| {
                user.as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a string PackageUser, got {user:?}")
                    })
                    .to_string()
            })
            .collect(),
        Some(other) => panic!("case {case}: expected an AllowListedUserList array, got {other:?}"),
    }
}

/// The `PackageUserList` of an accepted scope update.
fn answered_scope(answer: &Answer, case: &str) -> Vec<String> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageUserList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageUserList array, got {:?}",
                answer.body
            )
        })
        .iter()
        .map(|user| {
            user.as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a string PackageUser, got {user:?}")
                })
                .to_string()
        })
        .collect()
}

/// Admit this account to package vending, which no operation of this API does.
fn entitle(emulator: &Emulator) {
    let store = emulator.store();
    let mut guard = store.lock();
    guard.package_vending_entitled = true;
}

/// Assert `answer` was refused with `status` and `code`, naming `case`.
fn refused(answer: &Answer, case: &str, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected {status}, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        code,
        "case {case}: expected {code}, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn an_account_that_may_not_vend_packages_is_refused_every_scope_update() {
    let emulator = Emulator::new();
    let package =
        created_package(&emulator, "scope-unentitled", PACKAGE_TYPE_ZIP_PLUGIN, Some(true)).await;

    // Rows differ along what the rule generalises over: the operation asked for —
    // every value of `PackageScopeOperationEnum`, whose set comes from
    // `smithy_query(query="shape",shape="PackageScopeOperationEnum")` and is exported
    // as `PACKAGE_SCOPE_OPERATION_ENUM_VALUES` — and the users named: one account,
    // several, every user, none at all.
    let lists: Vec<Vec<&str>> = vec![
        vec![ACCOUNT_A],
        vec![ACCOUNT_A, ACCOUNT_B, ACCOUNT_C],
        vec![GLOBAL],
        vec![],
    ];

    for operation in PACKAGE_SCOPE_OPERATION_ENUM_VALUES {
        for users in &lists {
            let case = format!("UpdatePackageScope({operation}, {users:?}) unentitled");
            let answer = update_scope(&emulator, &package, operation, users).await;
            refused(&answer, &case, 400, "ValidationException");
            assert_eq!(
                answer.body["message"].as_str(),
                Some(NOT_AUTHORIZED),
                "case {case}: expected the entitlement refusal, got {:?}",
                answer.body
            );
            let scope = reported_scope(&emulator, &package, &case).await;
            assert!(
                scope.is_empty(),
                "case {case}: expected the refused call to share the package with nobody, \
                 got {scope:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_operation_applies_the_users_named_as_a_union_replacement_or_difference() {
    let emulator = Emulator::new();
    entitle(&emulator);
    let package = created_package(
        &emulator,
        "scope-semantics",
        PACKAGE_TYPE_ZIP_PLUGIN,
        Some(true),
    )
    .await;

    // One table of steps against ONE package, each row the operation to apply, the
    // users it names and the scope the package must hold afterwards. The rows differ
    // along what the rule generalises over: which operation, how many users the
    // request names, whether it repeats a user, whether the users are already in the
    // scope or absent from it, and the empty list at both ends.
    let steps: Vec<(&str, &str, Vec<&str>, Vec<&str>)> = vec![
        ("add-to-empty", "ADD", vec![ACCOUNT_A], vec![ACCOUNT_A]),
        (
            "add-a-second",
            "ADD",
            vec![ACCOUNT_B],
            vec![ACCOUNT_A, ACCOUNT_B],
        ),
        (
            "add-one-already-granted-and-one-new",
            "ADD",
            vec![ACCOUNT_A, ACCOUNT_C],
            vec![ACCOUNT_A, ACCOUNT_B, ACCOUNT_C],
        ),
        (
            "add-nobody",
            "ADD",
            vec![],
            vec![ACCOUNT_A, ACCOUNT_B, ACCOUNT_C],
        ),
        (
            "remove-a-user-that-was-never-granted",
            "REMOVE",
            vec![ACCOUNT_D],
            vec![ACCOUNT_A, ACCOUNT_B, ACCOUNT_C],
        ),
        (
            "remove-one",
            "REMOVE",
            vec![ACCOUNT_B],
            vec![ACCOUNT_A, ACCOUNT_C],
        ),
        (
            "override-with-a-different-set",
            "OVERRIDE",
            vec![GLOBAL, ACCOUNT_B],
            vec![GLOBAL, ACCOUNT_B],
        ),
        (
            "override-with-a-list-that-repeats-a-user",
            "OVERRIDE",
            vec![ACCOUNT_C, ACCOUNT_C, ACCOUNT_A],
            vec![ACCOUNT_C, ACCOUNT_A],
        ),
        (
            "add-a-list-that-repeats-a-user",
            "ADD",
            vec![ACCOUNT_B, ACCOUNT_B],
            vec![ACCOUNT_C, ACCOUNT_A, ACCOUNT_B],
        ),
        (
            "remove-several",
            "REMOVE",
            vec![ACCOUNT_C, ACCOUNT_B],
            vec![ACCOUNT_A],
        ),
        ("override-with-nobody", "OVERRIDE", vec![], vec![]),
    ];

    for (case, operation, users, expected) in steps {
        let answer = update_scope(&emulator, &package, operation, &users).await;
        let answered = answered_scope(&answer, case);
        assert_eq!(
            answered, expected,
            "case {case}: expected UpdatePackageScope({operation}, {users:?}) to answer \
             {expected:?}, got {answered:?}"
        );
        assert_eq!(
            (
                answer.body["PackageID"].as_str(),
                answer.body["Operation"].as_str(),
            ),
            (Some(package.as_str()), Some(operation)),
            "case {case}: expected the answer to name the package and the operation it \
             performed, got {:?}",
            answer.body
        );
        let reported = reported_scope(&emulator, &package, case).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected DescribePackages to report {expected:?} as the package's \
             AllowListedUserList, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_scope_update_that_grants_nothing_new_leaves_the_record_as_it_was() {
    let emulator = Emulator::new();
    entitle(&emulator);
    let package = created_package(
        &emulator,
        "scope-idempotent",
        PACKAGE_TYPE_ZIP_PLUGIN,
        Some(true),
    )
    .await;
    // Past the copy span, so the only thing that can move the record afterwards is a
    // call that changes it.
    tokio::time::sleep(SETTLE_WAIT).await;

    let granted = update_scope(&emulator, &package, "ADD", &[ACCOUNT_A, ACCOUNT_B]).await;
    assert_eq!(
        answered_scope(&granted, "grant"),
        vec![ACCOUNT_A, ACCOUNT_B],
        "case grant: expected the two accounts to be granted, got {:?}",
        granted.body
    );
    let stamped = reported_details(&emulator, &package, "grant").await["LastUpdatedAt"].clone();

    // Rows differ along the way a request can ask for a scope the package already has:
    // re-adding both users, re-adding one, overriding with the set it holds, and
    // removing a user it was never shared with.
    let unchanged: Vec<(&str, &str, Vec<&str>)> = vec![
        ("re-add-both", "ADD", vec![ACCOUNT_A, ACCOUNT_B]),
        ("re-add-one", "ADD", vec![ACCOUNT_B]),
        (
            "override-with-the-same-set",
            "OVERRIDE",
            vec![ACCOUNT_A, ACCOUNT_B],
        ),
        ("remove-a-user-never-granted", "REMOVE", vec![ACCOUNT_D]),
    ];

    for (case, operation, users) in unchanged {
        let answer = update_scope(&emulator, &package, operation, &users).await;
        let answered = answered_scope(&answer, case);
        assert_eq!(
            answered,
            vec![ACCOUNT_A, ACCOUNT_B],
            "case {case}: expected the scope to stay the two accounts granted, got {answered:?}"
        );
        let details = reported_details(&emulator, &package, case).await;
        assert_eq!(
            details["LastUpdatedAt"], stamped,
            "case {case}: expected a call that changes nothing to leave LastUpdatedAt at \
             {stamped:?}, got {:?}",
            details["LastUpdatedAt"]
        );
    }
}

#[tokio::test]
async fn only_a_package_whose_vending_is_enabled_can_be_shared() {
    let emulator = Emulator::new();
    entitle(&emulator);

    // Rows span every `PackageType` the model declares (`PACKAGE_TYPE_VALUES`, from
    // `smithy_query(query="shape",shape="PackageType")`) against all three forms the
    // vending container takes: absent, present and disabled, present and enabled.
    for package_type in PACKAGE_TYPE_VALUES {
        let suffix = package_type.to_ascii_lowercase().replace(['-', '_'], "");
        for (label, vending) in [
            ("absent", None),
            ("disabled", Some(false)),
            ("enabled", Some(true)),
        ] {
            let case = format!("UpdatePackageScope on a {package_type} package, vending {label}");
            let package = created_package(
                &emulator,
                &format!("scope-vending-{label}-{suffix}"),
                package_type,
                vending,
            )
            .await;
            let answer = update_scope(&emulator, &package, "ADD", &[ACCOUNT_A]).await;

            if vending == Some(true) {
                let answered = answered_scope(&answer, &case);
                assert_eq!(
                    answered,
                    vec![ACCOUNT_A],
                    "case {case}: expected the account to be granted, got {answered:?}"
                );
            } else {
                refused(&answer, &case, 400, "ValidationException");
                let scope = reported_scope(&emulator, &package, &case).await;
                assert!(
                    scope.is_empty(),
                    "case {case}: expected the refused call to share the package with nobody, \
                     got {scope:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_package_is_shared_with_at_most_the_bounded_number_of_users() {
    let emulator = Emulator::new();
    entitle(&emulator);
    let package = created_package(
        &emulator,
        "scope-bounded",
        PACKAGE_TYPE_ZIP_PLUGIN,
        Some(true),
    )
    .await;

    // Distinct 12-digit account ids, the form `PackageUser`'s pattern
    // `^[0-9]{12}$|^GLOBAL$` admits.
    let accounts: Vec<String> = (1..=PACKAGE_SCOPE_QUOTA + 1)
        .map(|n| format!("{n:012}"))
        .collect();
    let at_bound: Vec<&str> = accounts[..PACKAGE_SCOPE_QUOTA]
        .iter()
        .map(String::as_str)
        .collect();
    let past_bound: Vec<&str> = accounts.iter().map(String::as_str).collect();

    // One past the bound is refused, in one call and by adding to a scope already at
    // it, and the stored scope is left where it was in both.
    let answer = update_scope(&emulator, &package, "OVERRIDE", &past_bound).await;
    refused(&answer, "override-past-the-bound", 400, "ValidationException");
    let scope = reported_scope(&emulator, &package, "override-past-the-bound").await;
    assert!(
        scope.is_empty(),
        "case override-past-the-bound: expected the refused call to grant nothing, got \
         {} users",
        scope.len()
    );

    // The bound itself is accepted.
    let answer = update_scope(&emulator, &package, "OVERRIDE", &at_bound).await;
    let answered = answered_scope(&answer, "override-at-the-bound");
    assert_eq!(
        answered.len(),
        PACKAGE_SCOPE_QUOTA,
        "case override-at-the-bound: expected {PACKAGE_SCOPE_QUOTA} users, got {}",
        answered.len()
    );

    let answer = update_scope(&emulator, &package, "ADD", &past_bound[PACKAGE_SCOPE_QUOTA..]).await;
    refused(&answer, "add-past-the-bound", 400, "ValidationException");
    let scope = reported_scope(&emulator, &package, "add-past-the-bound").await;
    assert_eq!(
        scope.len(),
        PACKAGE_SCOPE_QUOTA,
        "case add-past-the-bound: expected the scope to stay at {PACKAGE_SCOPE_QUOTA} users, \
         got {}",
        scope.len()
    );

    // A request at the bound that withdraws access is accepted, whatever it names.
    let answer = update_scope(&emulator, &package, "REMOVE", &at_bound[..1]).await;
    let answered = answered_scope(&answer, "remove-at-the-bound");
    assert_eq!(
        answered.len(),
        PACKAGE_SCOPE_QUOTA - 1,
        "case remove-at-the-bound: expected {} users, got {}",
        PACKAGE_SCOPE_QUOTA - 1,
        answered.len()
    );
}

#[tokio::test]
async fn an_id_that_names_no_package_of_the_account_is_refused_as_a_bad_value() {
    let emulator = Emulator::new();
    entitle(&emulator);
    // A real package, so what the rows below are refused for is the id they name and
    // not an empty store.
    created_package(
        &emulator,
        "scope-present",
        PACKAGE_TYPE_ZIP_PLUGIN,
        Some(true),
    )
    .await;

    // Rows differ along the forms `PackageID`'s pattern `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$`
    // admits: both of the service-published forms and two of the created form.
    let ids = [UNKNOWN_PACKAGE_ID, "pkg-abc123", "F123456", "G7"];

    for id in ids {
        let case = format!("UpdatePackageScope({id:?})");
        let answer = update_scope(&emulator, id, "ADD", &[ACCOUNT_A]).await;
        refused(&answer, &case, 400, "ValidationException");
        assert_eq!(
            answer.body["message"].as_str(),
            Some(PACKAGE_NOT_FOUND),
            "case {case}: expected the fixed id-free sentence, got {:?}",
            answer.body
        );
    }
}
