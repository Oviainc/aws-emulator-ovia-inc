// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IDENTITY of a named grant — which `GrantId` `CreateGrant`
//! answers — driven over the wire through the shared [`super::harness`].
//!
//! The rule under test: a grant created with a `Name` has an id that is a FUNCTION of
//! the grant's parameters (the key, the grantee and retiring principals, the set of
//! permitted operations, the constraints and the name), so every request describing that
//! grant answers that one id — the byte-identical replay, the replay that lists the
//! operations in another order, and the request made again after `RevokeGrant` removed
//! the grant, where no record is left to read an id from. A grant created with NO name
//! has no such identity: "all CreateGrant requests result in a new grant with a unique
//! GrantId even if all the supplied parameters are identical"
//! (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`), so each such
//! call answers an id of its own.
//!
//! The rows differ along what the rule generalises over: which parameter of the grant
//! differs (the permitted operations, their order, the grantee principal, the retiring
//! principal, the encryption-context constraint, the name, the key), how many operations
//! the grant permits, and whether the grant is created before or after a revoke.
//!
//! The ids themselves are never asserted against a literal: a derived id is a digest of
//! parameters this test does not reproduce, so what every assertion is about is which
//! requests agree on an id and which do not. That a grant round-trips through `ListGrants`
//! is [`super::grants`]'s rule; this suite is only about the id.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{
    assert_empty, create_key, enabled_key, is_lower_hex, ok, principal, router, text,
};

/// The length of a grant id: KMS gives a grant 64 lowercase hexadecimal characters
/// (`GrantIdType`'s modelled `@length(1..128)` admits far more, so this is the form the
/// service mints).
const GRANT_ID_LEN: usize = 64;

/// One row of the parameter table: the case it stands for, and the `CreateGrant`
/// parameters it adds to the key under test.
struct Row {
    case: &'static str,
    parameters: Value,
}

/// The grant parameter sets the rule is asserted over. Every row carries a `Name`, so
/// every row's id is a derived one; the rows differ in each parameter a grant's identity
/// is made of, including two that differ only in the ORDER of the permitted operations
/// (the same SET, so the same grant) and two that differ only in their name.
fn rows() -> Vec<Row> {
    let grantee = principal("user/grantee");
    let other_grantee = principal("role/other-grantee");
    let retiring = principal("role/retirer");
    vec![
        Row {
            case: "one permitted operation",
            parameters: json!({
                "Name": "one-operation",
                "GranteePrincipal": grantee,
                "Operations": ["Encrypt"],
            }),
        },
        Row {
            case: "the same name with a different operation set",
            parameters: json!({
                "Name": "one-operation",
                "GranteePrincipal": grantee,
                "Operations": ["Decrypt"],
            }),
        },
        Row {
            case: "several permitted operations",
            parameters: json!({
                "Name": "several-operations",
                "GranteePrincipal": grantee,
                "Operations": ["Encrypt", "Decrypt", "DescribeKey"],
            }),
        },
        Row {
            case: "the same operations in another order",
            parameters: json!({
                "Name": "several-operations",
                "GranteePrincipal": grantee,
                "Operations": ["DescribeKey", "Decrypt", "Encrypt"],
            }),
        },
        Row {
            case: "another grantee principal",
            parameters: json!({
                "Name": "several-operations",
                "GranteePrincipal": other_grantee,
                "Operations": ["Encrypt", "Decrypt", "DescribeKey"],
            }),
        },
        Row {
            case: "a retiring principal as well",
            parameters: json!({
                "Name": "with-retiring",
                "GranteePrincipal": grantee,
                "RetiringPrincipal": retiring,
                "Operations": ["Encrypt"],
            }),
        },
        Row {
            case: "an encryption-context constraint",
            parameters: json!({
                "Name": "with-constraint",
                "GranteePrincipal": grantee,
                "Operations": ["Encrypt", "Decrypt"],
                "Constraints": {"EncryptionContextSubset": {"purpose": "test"}},
            }),
        },
        Row {
            case: "a different constraint under the same name",
            parameters: json!({
                "Name": "with-constraint",
                "GranteePrincipal": grantee,
                "Operations": ["Encrypt", "Decrypt"],
                "Constraints": {"EncryptionContextEquals": {"purpose": "test"}},
            }),
        },
        Row {
            case: "another name for otherwise identical parameters",
            parameters: json!({
                "Name": "with-another-name",
                "GranteePrincipal": grantee,
                "Operations": ["Encrypt"],
            }),
        },
    ]
}

/// The request a row describes against `key_id`.
fn request(key_id: &str, parameters: &Value) -> Value {
    let mut body = json!({"KeyId": key_id});
    for (member, value) in parameters
        .as_object()
        .expect("every row states its parameters as an object")
    {
        body[member] = value.clone();
    }
    body
}

/// Create a grant, requiring success, and answer the `GrantId` — checked to have the
/// form a grant id takes.
async fn grant_id(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let answer = ok(router, "CreateGrant", body).await;
    let grant_id = text(case, &answer, "GrantId");
    assert!(
        grant_id.len() == GRANT_ID_LEN && is_lower_hex(&grant_id),
        "case {case:?}: expected a grant id of {GRANT_ID_LEN} lowercase hex characters, \
         got {grant_id:?}"
    );
    grant_id
}

/// Revoke the grant `grant_id` names on `key_id`, requiring the empty 200.
async fn revoke(router: &MultiServiceRouter, case: &str, key_id: &str, grant_id: &str) {
    let answer = ok(
        router,
        "RevokeGrant",
        &json!({"KeyId": key_id, "GrantId": grant_id}),
    )
    .await;
    assert_empty(case, "RevokeGrant", &answer);
}

/// A named grant's id is a function of its parameters: the same parameters answer the
/// same id however often the grant is created, including after it was revoked, and
/// parameters that describe another grant answer another id.
#[tokio::test]
async fn a_named_grants_id_is_derived_from_its_parameters() {
    let router = router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");

    let mut by_case: Vec<(&str, String)> = Vec::new();
    for row in rows() {
        let body = request(&key_id, &row.parameters);

        // The first create, the byte-identical replay and the revoke/recreate cycle all
        // answer ONE id: the replay is the documented idempotent retry, and the recreate
        // has no record left to read an id from, so only the parameters can decide it.
        let first = grant_id(&router, row.case, &body).await;
        let replayed = grant_id(&router, row.case, &body).await;
        assert_eq!(
            replayed, first,
            "case {:?}: a replay of a named grant must answer the id the grant has, got \
             {replayed} after {first}",
            row.case
        );

        revoke(&router, row.case, &key_id, &first).await;
        let recreated = grant_id(&router, row.case, &body).await;
        assert_eq!(
            recreated, first,
            "case {:?}: recreating a revoked named grant must answer its id {first}, got \
             {recreated}",
            row.case
        );

        by_case.push((row.case, first));
    }

    // Rows that describe the SAME grant share an id; every other pair of rows does not.
    // The operations are a SET of permissions, so the row that reorders them is the same
    // grant as the one it reorders.
    let same: &[(&str, &str)] = &[(
        "several permitted operations",
        "the same operations in another order",
    )];
    for (left_case, left_id) in &by_case {
        for (right_case, right_id) in &by_case {
            if left_case == right_case {
                continue;
            }
            let describes_one_grant = same.contains(&(left_case, right_case))
                || same.contains(&(right_case, left_case));
            assert_eq!(
                left_id == right_id,
                describes_one_grant,
                "case {left_case:?} vs {right_case:?}: expected the same id: \
                 {describes_one_grant}, got {left_id} and {right_id}"
            );
        }
    }
}

/// The id is derived from the KEY as well, so the same named grant on two keys is two
/// grants with two ids — and a key that holds a named grant still reports it after the
/// same grant is created on another key.
#[tokio::test]
async fn the_same_named_grant_on_another_key_has_an_id_of_its_own() {
    let router = router();
    let first_key = text("setup", &enabled_key(&router).await, "KeyId");
    let second_key = text(
        "setup",
        &create_key(&router, &json!({"Description": "another key"})).await,
        "KeyId",
    );

    let parameters = json!({
        "Name": "shared-name",
        "GranteePrincipal": principal("user/grantee"),
        "Operations": ["Encrypt"],
    });
    let on_first = grant_id(&router, "the first key", &request(&first_key, &parameters)).await;
    let on_second = grant_id(&router, "the second key", &request(&second_key, &parameters)).await;
    assert_ne!(
        on_first, on_second,
        "case two keys: a named grant on another key is another grant, got {on_first} twice"
    );

    for (case, key_id, expected) in [
        ("the first key", &first_key, &on_first),
        ("the second key", &second_key, &on_second),
    ] {
        let listed = ok(&router, "ListGrants", &json!({"KeyId": key_id})).await;
        let ids: Vec<String> = listed["Grants"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a Grants list in {listed}"))
            .iter()
            .filter_map(|grant| grant["GrantId"].as_str().map(str::to_string))
            .collect();
        assert_eq!(
            ids,
            vec![expected.clone()],
            "case {case}: the key holds exactly its own named grant, got {ids:?}"
        );
    }
}

/// A grant created with NO name has no derived identity: every call creates a grant of
/// its own, with an id no other call answers.
#[tokio::test]
async fn an_unnamed_grant_gets_a_new_id_every_time() {
    let router = router();
    let key_id = text("setup", &enabled_key(&router).await, "KeyId");

    // Rows: how many of the identical creates are made, so the rule is asserted over a
    // run of them rather than over one pair.
    let body = json!({
        "KeyId": key_id,
        "GranteePrincipal": principal("user/grantee"),
        "Operations": ["Encrypt"],
    });
    let mut ids: BTreeSet<String> = BTreeSet::new();
    for attempt in 0..4 {
        let case = format!("unnamed grant {attempt}");
        let id = grant_id(&router, &case, &body).await;
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: an unnamed grant is a new grant every time, got {id} again"
        );
    }

    let listed = ok(&router, "ListGrants", &json!({"KeyId": key_id})).await;
    let reported: BTreeSet<String> = listed["Grants"]
        .as_array()
        .unwrap_or_else(|| panic!("case every unnamed grant: expected a list in {listed}"))
        .iter()
        .filter_map(|grant| grant["GrantId"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        reported, ids,
        "case every unnamed grant: the key holds one grant per call, got {reported:?}"
    );
}
