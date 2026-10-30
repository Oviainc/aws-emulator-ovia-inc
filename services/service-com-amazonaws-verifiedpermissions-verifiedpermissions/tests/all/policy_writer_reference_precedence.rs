// SPDX-License-Identifier: CC0-1.0

//! Which refusal the POLICY WRITERS answer for a request that is faulty in TWO
//! ways at once: it references a resource this service does not hold AND it
//! carries a definition the operation refuses on its own checks.
//!
//! The rule under test is the one
//! [`crate::not_found_precedes_payload_refusal`] pins for the authorization
//! calls and the identity-source writers: an operation resolves every resource
//! the request REFERENCES before it reads the request's own payload, so a
//! reference that names nothing answers the 404 `ResourceNotFoundException`
//! whatever else the request got wrong — the same answer whichever operation the
//! reference arrived on, and whichever payload fault accompanies it. Against the
//! resources that DO exist the very same payload earns its 400
//! `ValidationException` naming the member at fault, so what is pinned is the
//! PRECEDENCE and not a blanket 404.
//!
//! `CreatePolicy` and `UpdatePolicy` are the writers that read caller-sized Cedar
//! text, so a size refusal is one of the payload faults the rows span here; the
//! bound itself is pinned by the second test, which keeps it enforced from its
//! place after the references are resolved.
//!
//! Every assertion goes over the wire through the shared [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_verifiedpermissions_verifiedpermissions::state::cedar::MAX_STATEMENT_BYTES;

use crate::harness::{call, error_code, ok, router, store_with_validation_off};

/// A policy store id no store in a fresh engine carries.
const UNKNOWN_STORE: &str = "PSdoesnotexist11111111";

/// The prefix a `policyStoreId` carries when it references a store by ALIAS,
/// from `CreatePolicyStoreAliasInput$aliasName`'s model documentation: "The alias
/// name must always be prefixed with `policy-store-alias/`."
const ALIAS_PREFIX: &str = "policy-store-alias/";

/// The alias name no alias in a fresh engine carries.
const UNKNOWN_ALIAS_NAME: &str = "not-an-alias";

/// A policy id no store holds.
const UNKNOWN_POLICY: &str = "SPdoesnotexist11111111";

/// The prefix a `policyId` carries when it references a policy by NAME, from
/// `UpdatePolicyInput$policyId`'s model documentation: "You can use the policy
/// name in place of the policy ID. When using a name, prefix it with `name/`."
const POLICY_NAME_PREFIX: &str = "name/";

/// The policy name no policy holds.
const UNKNOWN_POLICY_NAME: &str = "no-such-policy";

/// The policy the live store really holds: a scope naming all three positions, so
/// a replacement can keep it or change one of them.
const STORED: &str = "permit(principal == PhotoApp::User::\"alice\", \
                      action == PhotoApp::Action::\"viewPhoto\", \
                      resource == PhotoApp::Photo::\"vacation.jpg\");";

/// [`STORED`]'s own scope, padded inside a condition clause to exactly `bytes`
/// bytes: the only thing that differs between two of these is the SIZE, so a row
/// about the size bound changes nothing an update may not change.
fn padded_to(bytes: usize) -> String {
    let prefix = format!("{} when {{ context.tag == \"", STORED.trim_end_matches(';'));
    let suffix = "\" };";
    let padding = bytes.saturating_sub(prefix.len() + suffix.len());
    format!("{prefix}{}{suffix}", "x".repeat(padding))
}

/// A replacement that keeps [`STORED`]'s effect and resource and changes its
/// PRINCIPAL — an element an update may not change, so the definition is refused
/// on the operation's own rule rather than on anything about its size.
fn a_changed_principal() -> String {
    STORED.replace("alice", "bob")
}

/// Cedar TEMPLATE text, which the static-policy parser refuses: `?principal` is a
/// slot, so this is no static policy at all.
const TEMPLATE_TEXT: &str = "permit(principal == ?principal, \
                             action == PhotoApp::Action::\"viewPhoto\", \
                             resource == PhotoApp::Photo::\"vacation.jpg\");";

/// One request whose DEFINITION this service refuses on its own checks.
struct Fault {
    /// The operation to call.
    operation: &'static str,
    /// What is wrong with the definition, for the case name.
    fault: &'static str,
    /// The member `ValidationException$fieldList` must name once every
    /// referenced resource resolves.
    field: &'static str,
    /// The request body, against the `policyStoreId` and `policyId` the row
    /// supplies.
    body: fn(&str, &str) -> Value,
}

/// Every definition refusal this suite pairs with a reference that names
/// nothing.
///
/// The rows differ along the OPERATION (the create and the update) and along the
/// KIND of refusal the definition earns: one byte past the size bound, far past
/// it, text that is no Cedar at all, text that is a template rather than a static
/// policy, and — for the update — a replacement that changes an element the
/// operation documents as immutable.
const FAULTS: &[Fault] = &[
    Fault {
        operation: "CreatePolicy",
        fault: "a statement one byte past the size bound",
        field: "/definition/static/statement",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "definition": { "static": { "statement": padded_to(MAX_STATEMENT_BYTES + 1) } },
            })
        },
    },
    Fault {
        operation: "CreatePolicy",
        fault: "a statement far past the size bound",
        field: "/definition/static/statement",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "definition": { "static": { "statement": padded_to(MAX_STATEMENT_BYTES * 8) } },
            })
        },
    },
    Fault {
        operation: "CreatePolicy",
        fault: "a statement that is not Cedar",
        field: "/definition/static/statement",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "definition": { "static": { "statement": "this is not cedar" } },
            })
        },
    },
    Fault {
        operation: "CreatePolicy",
        fault: "template text where a static policy belongs",
        field: "/definition/static/statement",
        body: |store, _| {
            json!({
                "policyStoreId": store,
                "definition": { "static": { "statement": TEMPLATE_TEXT } },
            })
        },
    },
    Fault {
        operation: "UpdatePolicy",
        fault: "a replacement one byte past the size bound",
        field: "/definition/static/statement",
        body: |store, policy| {
            json!({
                "policyStoreId": store,
                "policyId": policy,
                "definition": { "static": { "statement": padded_to(MAX_STATEMENT_BYTES + 1) } },
            })
        },
    },
    Fault {
        operation: "UpdatePolicy",
        fault: "a replacement far past the size bound",
        field: "/definition/static/statement",
        body: |store, policy| {
            json!({
                "policyStoreId": store,
                "policyId": policy,
                "definition": { "static": { "statement": padded_to(MAX_STATEMENT_BYTES * 8) } },
            })
        },
    },
    Fault {
        operation: "UpdatePolicy",
        fault: "a replacement that is not Cedar",
        field: "/definition/static/statement",
        body: |store, policy| {
            json!({
                "policyStoreId": store,
                "policyId": policy,
                "definition": { "static": { "statement": "this is not cedar" } },
            })
        },
    },
    Fault {
        operation: "UpdatePolicy",
        fault: "a replacement that changes the principal",
        field: "/definition/static/statement",
        body: |store, policy| {
            json!({
                "policyStoreId": store,
                "policyId": policy,
                "definition": { "static": { "statement": a_changed_principal() } },
            })
        },
    },
];

/// What a row expects of the call.
struct Expect {
    status: u16,
    code: &'static str,
    /// For a 404, the `(resourceType, resourceId)` the exception must name; for a
    /// 400, `None`.
    resource: Option<(&'static str, String)>,
}

/// One reference pair a row is sent with, and what it earns.
struct Reference {
    case: &'static str,
    store: String,
    policy: String,
    expect: Expect,
}

/// A reference that resolves to nothing outranks every refusal of the request's
/// own definition, on both policy writers — and the same definition against the
/// resources that DO exist still earns its 400 naming the member at fault.
#[tokio::test]
async fn an_unresolvable_reference_outranks_a_definition_refusal() {
    let router = router();
    let live_store = store_with_validation_off(&router).await;
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": live_store,
            "definition": { "static": { "statement": STORED } },
        }),
    )
    .await;
    let live_policy = created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    for fault in FAULTS {
        // The rows differ along WHAT the request's references name: no store at
        // all, an alias naming no store, the store that exists paired with a
        // policy that does not (by id and by name), and the resources that both
        // exist.
        let mut references = vec![
            Reference {
                case: "a store id no store carries",
                store: UNKNOWN_STORE.to_string(),
                policy: UNKNOWN_POLICY.to_string(),
                expect: Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    resource: Some(("POLICY_STORE", UNKNOWN_STORE.to_string())),
                },
            },
            Reference {
                case: "an alias naming nothing",
                store: format!("{ALIAS_PREFIX}{UNKNOWN_ALIAS_NAME}"),
                policy: UNKNOWN_POLICY.to_string(),
                expect: Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    // The reference named an ALIAS, so the alias is the resource
                    // reported missing, by its bare name.
                    resource: Some(("POLICY_STORE_ALIAS", UNKNOWN_ALIAS_NAME.to_string())),
                },
            },
            Reference {
                case: "the resources that do exist",
                store: live_store.clone(),
                policy: live_policy.clone(),
                expect: Expect {
                    status: 400,
                    code: "ValidationException",
                    resource: None,
                },
            },
        ];
        if fault.operation == "UpdatePolicy" {
            // The SECOND resource an update references: the policy itself, which
            // is resolved before the definition is read just as the store is.
            references.push(Reference {
                case: "a policy id the store does not hold",
                store: live_store.clone(),
                policy: UNKNOWN_POLICY.to_string(),
                expect: Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    resource: Some(("POLICY", UNKNOWN_POLICY.to_string())),
                },
            });
            references.push(Reference {
                case: "a policy name the store does not hold",
                store: live_store.clone(),
                policy: format!("{POLICY_NAME_PREFIX}{UNKNOWN_POLICY_NAME}"),
                expect: Expect {
                    status: 404,
                    code: "ResourceNotFoundException",
                    // The reference named a policy by NAME, so the bare name is
                    // the id reported missing.
                    resource: Some(("POLICY", UNKNOWN_POLICY_NAME.to_string())),
                },
            });
        }

        for reference in references {
            let case = format!(
                "{} with {} against {}",
                fault.operation, fault.fault, reference.case
            );
            let body = (fault.body)(&reference.store, &reference.policy);
            let (status, response) = call(&router, fault.operation, &body).await;
            assert_eq!(
                status, reference.expect.status,
                "case {case:?}: expected HTTP {}, got {status} {response}",
                reference.expect.status
            );
            assert_eq!(
                error_code(&response),
                reference.expect.code,
                "case {case:?}: expected {}, got {response}",
                reference.expect.code
            );
            match &reference.expect.resource {
                Some((resource_type, resource_id)) => {
                    assert_eq!(
                        response["resourceType"].as_str(),
                        Some(*resource_type),
                        "case {case:?}: expected resourceType {resource_type}, got {response}"
                    );
                    assert_eq!(
                        response["resourceId"].as_str(),
                        Some(resource_id.as_str()),
                        "case {case:?}: expected resourceId {resource_id}, got {response}"
                    );
                }
                None => {
                    let paths: Vec<&str> = response["fieldList"]
                        .as_array()
                        .map(|fields| {
                            fields
                                .iter()
                                .filter_map(|field| field["path"].as_str())
                                .collect()
                        })
                        .unwrap_or_default();
                    assert!(
                        paths.contains(&fault.field),
                        "case {case:?}: expected fieldList to name {}, got {paths:?} in {response}",
                        fault.field
                    );
                }
            }
        }
    }

    // No refusal reached state: the store still holds exactly the one policy the
    // suite seeded, carrying the text it was created with.
    let listed = ok(
        &router,
        "ListPolicies",
        &json!({ "policyStoreId": live_store }),
    )
    .await;
    let ids: Vec<&str> = listed["policies"]
        .as_array()
        .map(|policies| {
            policies
                .iter()
                .filter_map(|policy| policy["policyId"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        ids,
        vec![live_policy.as_str()],
        "case \"a refused write changes nothing\": expected only the seeded policy, got {listed}"
    );
    let fetched = ok(
        &router,
        "GetPolicy",
        &json!({ "policyStoreId": live_store, "policyId": live_policy }),
    )
    .await;
    assert_eq!(
        fetched["definition"]["static"]["statement"].as_str(),
        Some(STORED),
        "case \"a refused write changes nothing\": the stored statement must be unchanged, got \
         {fetched}"
    );
}

/// The size bound still decides, from its place after the references are
/// resolved: a statement AT the bound is accepted by both writers and read back,
/// one byte past it and far past it are refused, and the refusal leaves the
/// stored policy alone.
#[tokio::test]
async fn the_statement_size_bound_still_decides_once_the_references_resolve() {
    let router = router();
    let store = store_with_validation_off(&router).await;
    let created = ok(
        &router,
        "CreatePolicy",
        &json!({
            "policyStoreId": store,
            "definition": { "static": { "statement": STORED } },
        }),
    )
    .await;
    let policy = created["policyId"]
        .as_str()
        .expect("policyId is required on the CreatePolicy response")
        .to_string();

    // (case, the statement's size in bytes, the status both writers must answer)
    // — the published bound itself, one byte past it, and far past it.
    let sizes: &[(&str, usize, u16)] = &[
        ("at the bound", MAX_STATEMENT_BYTES, 200),
        ("one byte past the bound", MAX_STATEMENT_BYTES + 1, 400),
        ("far past the bound", MAX_STATEMENT_BYTES * 8, 400),
    ];
    for (size_case, bytes, expected) in sizes {
        let statement = padded_to(*bytes);
        assert_eq!(
            statement.len(),
            *bytes,
            "case {size_case:?}: the padded statement should be {bytes} bytes, got {}",
            statement.len()
        );

        let case = format!("CreatePolicy with a statement {size_case}");
        let (status, response) = call(
            &router,
            "CreatePolicy",
            &json!({
                "policyStoreId": store,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if *expected != 200 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response}"
            );
        }

        let case = format!("UpdatePolicy with a replacement {size_case}");
        let (status, response) = call(
            &router,
            "UpdatePolicy",
            &json!({
                "policyStoreId": store,
                "policyId": policy,
                "definition": { "static": { "statement": statement } },
            }),
        )
        .await;
        assert_eq!(
            status, *expected,
            "case {case:?}: expected HTTP {expected}, got {status} {response}"
        );
        if *expected == 200 {
            // The accepted replacement really reached state.
            let fetched = ok(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store, "policyId": policy }),
            )
            .await;
            assert_eq!(
                fetched["definition"]["static"]["statement"].as_str(),
                Some(statement.as_str()),
                "case {case:?}: the accepted replacement must be read back, got {fetched}"
            );
        } else {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response}"
            );
            // The refused replacement left the policy carrying what the last
            // accepted write put there.
            let fetched = ok(
                &router,
                "GetPolicy",
                &json!({ "policyStoreId": store, "policyId": policy }),
            )
            .await;
            assert_eq!(
                fetched["definition"]["static"]["statement"].as_str(),
                Some(padded_to(MAX_STATEMENT_BYTES).as_str()),
                "case {case:?}: a refused replacement must change nothing, got {fetched}"
            );
        }
    }
}
