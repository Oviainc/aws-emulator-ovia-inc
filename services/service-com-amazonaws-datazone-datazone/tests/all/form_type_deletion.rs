// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `DeleteFormType`.
//!
//! Every case drives the service the way a client does — a real `restJson1` request
//! through the router — and uses only the responses as evidence, so a handler that
//! answered from a hardcoded value instead of from the store fails here.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one request:
//!
//! 1. Deleting a stored metadata form type that nothing uses removes the whole record:
//!    the call answers 204 and every revision of it stops being readable, so a second
//!    delete of the same identifier is a miss.
//! 2. "The form type must not be in use by any asset types or assets" / "Any
//!    dependencies (such as linked asset types) must be removed first"
//!    (`smithy_query(query="doc",shape="DeleteFormType")`): a form type a stored asset
//!    type or asset binds a metadata form to is refused with `ConflictException` naming
//!    the form type and the object that prevented the delete, and the refusal stores
//!    nothing. A form reference is resolved to the form type NAME it addresses, so a
//!    qualified identifier blocks the form type its last segment names and a reference
//!    to any other form type does not block at all.
//! 3. A delete is scoped to the domain in the request path and addresses a form type by
//!    the identifier that reads it, so a name stored in another domain, or in no domain,
//!    is a `ResourceNotFoundException` naming the KIND and never the caller's
//!    identifier.

use crate::harness::{Emulator, SETTLED};
use serde_json::{json, Map, Value};

/// `FormTypeName`'s modelled maximum length, 1..128 (smithy_query shape="FormTypeName").
const NAME_MAX: usize = 128;

/// `FormTypeIdentifier`'s modelled maximum length, 1..385 (smithy_query
/// shape="FormTypeIdentifier").
const IDENTIFIER_MAX: usize = 385;

/// The `ResourceNotFoundException` message the service answers for a metadata form type
/// it cannot find: recorded behaviour, and a fixed sentence naming the resource KIND
/// rather than the caller's identifier (`crate::state::FORM_TYPE_NOT_FOUND_MESSAGE`).
const FORM_TYPE_NOT_FOUND: &str =
    "The given FormType doesn't exist. Try creating FormType before accessing it.";

/// The same, for a domain the service cannot find.
const DOMAIN_NOT_FOUND: &str = "Domain not found";

/// One object as a deletion refusal names it: `type <tag>#<domainId>#<identifier>`.
/// SOURCE: the recorded `ConflictException` wording, which names the form type being
/// deleted `type Form#<domainId>#<formTypeName>` and the asset-domain object that
/// prevented it `type asset#<domainId>#<identifier>`.
fn form_type_object(domain: &str, name: &str) -> String {
    format!("type Form#{domain}#{name}")
}

/// [`form_type_object`], for an asset type or an asset.
fn asset_object(domain: &str, identifier: &str) -> String {
    format!("type asset#{domain}#{identifier}")
}

/// The `ConflictException` message a delete a stored dependency prevents is refused
/// with: recorded wording, built from the objects the call found, ending in a period and
/// a space.
fn deletion_prevented_message(object: &str, blocker: &str) -> String {
    format!("The following objects could not be deleted: {object} deletion prevented by {blocker}. ")
}

/// Create `revisions` revisions of the form type `name`: re-creating a name is the only
/// way a second revision arises (the model declares no `UpdateFormType`).
async fn form_type_with_revisions(
    dz: &Emulator,
    domain: &str,
    project: &str,
    name: &str,
    revisions: usize,
) {
    for revision in 1..=revisions {
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/form-types"),
                Some(json!({
                    "name": name,
                    "model": { "smithy": format!("structure F {{ v{revision}: String }}") },
                    "owningProjectIdentifier": project,
                })),
            )
            .await;
        assert_eq!(
            (created.status, created.s("revision").as_str()),
            (201, revision.to_string().as_str()),
            "creating revision {revision} of {name:?}: expected 201 and that revision, got {} {:?}",
            created.status,
            created.body
        );
    }
}

/// One form type to delete when nothing uses it. The rows differ along the form of the
/// name (plain, underscore-led, digits, the service's own qualified namespace, the
/// modelled maximum length) and along how many revisions are stored under it.
struct UnusedCase {
    label: &'static str,
    name: String,
    revisions: usize,
}

fn unused_cases() -> Vec<UnusedCase> {
    vec![
        UnusedCase {
            label: "a plain name at one revision",
            name: "OrderForm".to_string(),
            revisions: 1,
        },
        UnusedCase {
            label: "an underscore-led name at two revisions",
            name: "_order_form_2".to_string(),
            revisions: 2,
        },
        UnusedCase {
            label: "digits and mixed case at three revisions",
            name: "Fin2Form_v3".to_string(),
            revisions: 3,
        },
        UnusedCase {
            // The modelled pattern admits an `amazon.datazone.` prefix, which puts dots
            // in the path segment the delete addresses the form type by.
            label: "a qualified name behind the amazon.datazone. prefix",
            name: "amazon.datazone.QualifiedForm".to_string(),
            revisions: 2,
        },
        UnusedCase {
            label: "a name at the modelled maximum length",
            name: format!("F{}", "o".repeat(NAME_MAX - 1)),
            revisions: 1,
        },
        UnusedCase {
            label: "an underscore followed only by digits, at five revisions",
            name: "_123".to_string(),
            revisions: 5,
        },
    ]
}

#[tokio::test]
async fn deleting_an_unused_form_type_removes_every_revision_of_it() {
    let dz = Emulator::new();
    let domain = dz.available_domain("form-type-delete-domain").await;
    let project = dz.project(&domain, "delete owner").await;

    for case in unused_cases() {
        form_type_with_revisions(&dz, &domain, &project, &case.name, case.revisions).await;
        let uri = format!("/v2/domains/{domain}/form-types/{}", case.name);

        // It is readable before the delete, so the 404 afterwards is evidence of the
        // delete rather than of a create that never landed.
        let before = dz.call("GET", &uri, None).await;
        assert_eq!(
            (before.status, before.s("revision").as_str()),
            (200, case.revisions.to_string().as_str()),
            "case {:?}: expected the stored form type at revision {}, got {} {:?}",
            case.label,
            case.revisions,
            before.status,
            before.body
        );

        let deleted = dz.call("DELETE", &uri, None).await;
        assert_eq!(
            deleted.status, 204,
            "case {:?}: expected 204 from DeleteFormType, got {} {:?}",
            case.label, deleted.status, deleted.body
        );
        assert_eq!(
            deleted.body.as_object().map(Map::len),
            Some(0),
            "case {:?}: DeleteFormType models no output, so the answer carries no members, \
             got {:?}",
            case.label,
            deleted.body
        );

        // The whole record went: the latest read and every revision that was stored.
        let after = dz.call("GET", &uri, None).await;
        assert_eq!(
            (after.status, after.error_code.as_str(), after.s("message")),
            (
                404,
                "ResourceNotFoundException",
                FORM_TYPE_NOT_FOUND.to_string()
            ),
            "case {:?}: a deleted form type must not be readable, got {} {:?}",
            case.label,
            after.status,
            after.body
        );
        for revision in 1..=case.revisions {
            let gone = dz.call("GET", &format!("{uri}?revision={revision}"), None).await;
            assert_eq!(
                (gone.status, gone.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {:?}: revision {revision} must go with the form type, got {} {:?}",
                case.label,
                gone.status,
                gone.body
            );
        }

        // A second delete of the same identifier is a miss, naming the KIND only.
        let again = dz.call("DELETE", &uri, None).await;
        assert_eq!(
            (again.status, again.error_code.as_str(), again.s("message")),
            (
                404,
                "ResourceNotFoundException",
                FORM_TYPE_NOT_FOUND.to_string()
            ),
            "case {:?}: expected a second delete to miss, got {} {:?}",
            case.label,
            again.status,
            again.body
        );
        assert!(
            !again.s("message").contains(&case.name),
            "case {:?}: a not-found message must not echo the caller's identifier, got {:?}",
            case.label,
            again.s("message")
        );
    }

    // Re-creating a deleted name mints a form type again, and its revisions start over:
    // nothing of the deleted record is left to continue from.
    form_type_with_revisions(&dz, &domain, &project, "OrderForm", 1).await;
    let remade = dz
        .call("GET", &format!("/v2/domains/{domain}/form-types/OrderForm"), None)
        .await;
    assert_eq!(
        (remade.status, remade.s("revision").as_str()),
        (200, "1"),
        "a deleted name must be creatable again at revision 1, got {} {:?}",
        remade.status,
        remade.body
    );
}

/// Which kind of stored resource holds the metadata form that references a form type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum User {
    /// An asset type whose `formsInput` entry names the form type.
    AssetType,
    /// An asset whose `formsInput` entry names the form type.
    Asset,
}

/// One form type, and a stored resource that references a form type. The rows differ
/// along which KIND of resource holds the reference, how the reference is SPELLED (the
/// name itself, or a qualified identifier whose last segment is that name), how many
/// forms the referencing resource carries and where in them the reference sits — and
/// along whether the reference names this form type at all, which is what decides
/// between the refusal and the 204.
struct InUseCase {
    label: &'static str,
    /// The form type that is created and then deleted.
    form_type: &'static str,
    user: User,
    /// Every `typeIdentifier` the referencing resource's forms carry.
    references: Vec<&'static str>,
    /// Whether the stored resource references THIS form type, which is what decides
    /// between the refusal and the 204.
    expect_blocked: bool,
}

fn in_use_cases() -> Vec<InUseCase> {
    vec![
        InUseCase {
            label: "an asset type whose only form names the form type by name",
            form_type: "OrdersFormType",
            user: User::AssetType,
            references: vec!["OrdersFormType"],
            expect_blocked: true,
        },
        InUseCase {
            label: "an asset type whose third of three forms names it",
            form_type: "ThirdFormType",
            user: User::AssetType,
            references: vec!["SomethingElse", "_private1", "ThirdFormType"],
            expect_blocked: true,
        },
        InUseCase {
            // A form entry's `typeIdentifier` may be qualified; the form type's own name
            // is the unqualified last segment, so the reference is to THIS form type.
            label: "an asset type naming it through a qualified identifier",
            form_type: "LeafFormType",
            user: User::AssetType,
            references: vec!["mydomain.sub.LeafFormType"],
            expect_blocked: true,
        },
        InUseCase {
            label: "an asset type naming it in the service's own namespace",
            form_type: "amazon.datazone.BuiltInLikeFormType",
            user: User::AssetType,
            references: vec!["amazon.datazone.BuiltInLikeFormType"],
            expect_blocked: true,
        },
        InUseCase {
            label: "an asset whose only form names the form type by name",
            form_type: "AssetHeldFormType",
            user: User::Asset,
            references: vec!["AssetHeldFormType"],
            expect_blocked: true,
        },
        InUseCase {
            label: "an asset naming it through a qualified identifier, among three forms",
            form_type: "DeepFormType",
            user: User::Asset,
            references: vec!["other.FirstFormType", "ns.deeper.DeepFormType", "Third"],
            expect_blocked: true,
        },
        InUseCase {
            label: "an asset type that references only OTHER form types",
            form_type: "UnreferencedByTypes",
            user: User::AssetType,
            references: vec!["SomeOtherFormType", "UnreferencedByTypesToo"],
            expect_blocked: false,
        },
        InUseCase {
            label: "an asset type whose reference only shares the last segment's prefix",
            form_type: "Orders",
            user: User::AssetType,
            references: vec!["ns.OrdersExtended"],
            expect_blocked: false,
        },
        InUseCase {
            label: "an asset that references only another form type",
            form_type: "UnreferencedByAssets",
            user: User::Asset,
            references: vec!["amazon.datazone.RelationalTableFormType"],
            expect_blocked: false,
        },
    ]
}

/// Store the referencing resource of one row, and answer the identifier a refusal names
/// it by: an asset type's NAME, or an asset's minted id.
async fn store_referencing_resource(
    dz: &Emulator,
    domain: &str,
    project: &str,
    case: &InUseCase,
) -> String {
    match case.user {
        User::AssetType => {
            let mut forms = Map::new();
            for (index, reference) in case.references.iter().enumerate() {
                forms.insert(
                    format!("form{index}"),
                    json!({ "typeIdentifier": reference, "typeRevision": "1" }),
                );
            }
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/asset-types"),
                    Some(json!({
                        "name": format!("TypeUsing{}", case.form_type.replace('.', "_")),
                        "owningProjectIdentifier": project,
                        "formsInput": Value::Object(forms),
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {:?}: CreateAssetType expected 201, got {} {:?}",
                case.label, created.status, created.body
            );
            created.s("name")
        }
        User::Asset => {
            let forms: Vec<Value> = case
                .references
                .iter()
                .enumerate()
                .map(|(index, reference)| {
                    json!({
                        "formName": format!("form{index}"),
                        "typeIdentifier": reference,
                        "content": "{}",
                    })
                })
                .collect();
            let created = dz
                .call(
                    "POST",
                    &format!("/v2/domains/{domain}/assets"),
                    Some(json!({
                        "name": format!("asset using {}", case.form_type),
                        "typeIdentifier": "amazon.datazone.RelationalTable",
                        "owningProjectIdentifier": project,
                        "formsInput": forms,
                    })),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {:?}: CreateAsset expected 201, got {} {:?}",
                case.label, created.status, created.body
            );
            created.s("id")
        }
    }
}

#[tokio::test]
async fn a_form_type_in_use_is_refused_and_an_unreferenced_one_is_deleted() {
    let dz = Emulator::new();
    let domain = dz.available_domain("in-use-domain").await;
    let project = dz.project(&domain, "in-use owner").await;

    for case in in_use_cases() {
        form_type_with_revisions(&dz, &domain, &project, case.form_type, 2).await;
        let blocker = store_referencing_resource(&dz, &domain, &project, &case).await;

        let uri = format!("/v2/domains/{domain}/form-types/{}", case.form_type);
        let answer = dz.call("DELETE", &uri, None).await;
        if case.expect_blocked {
            assert_eq!(
                (answer.status, answer.error_code.as_str()),
                (409, "ConflictException"),
                "case {:?}: expected 409 ConflictException, got {} {:?}",
                case.label,
                answer.status,
                answer.body
            );
            assert_eq!(
                answer.s("message"),
                deletion_prevented_message(
                    &form_type_object(&domain, case.form_type),
                    &asset_object(&domain, &blocker)
                ),
                "case {:?}: the refusal must name the form type and the stored object that \
                 prevented its deletion, got {:?}",
                case.label,
                answer.s("message")
            );
            // The refusal stored nothing: the form type is still there, at every
            // revision it had.
            for revision in ["1", "2"] {
                let still = dz.call("GET", &format!("{uri}?revision={revision}"), None).await;
                assert_eq!(
                    (still.status, still.s("revision")),
                    (200, revision.to_string()),
                    "case {:?}: a refused delete must leave revision {revision} readable, \
                     got {} {:?}",
                    case.label,
                    still.status,
                    still.body
                );
            }
        } else {
            assert_eq!(
                answer.status, 204,
                "case {:?}: a form type nothing references must be deleted, got {} {:?}",
                case.label, answer.status, answer.body
            );
            let after = dz.call("GET", &uri, None).await;
            assert_eq!(
                (after.status, after.error_code.as_str()),
                (404, "ResourceNotFoundException"),
                "case {:?}: the deleted form type must be gone, got {} {:?}",
                case.label,
                after.status,
                after.body
            );
        }
    }
}

#[tokio::test]
async fn a_refusal_reports_one_stable_dependency_preferring_the_asset_type() {
    let dz = Emulator::new();
    let domain = dz.available_domain("two-blockers-domain").await;
    let project = dz.project(&domain, "two-blockers owner").await;
    form_type_with_revisions(&dz, &domain, &project, "SharedFormType", 1).await;

    // Two asset types and one asset all bind a form to the same form type, each
    // referencing it a different way. The asset type is created SECOND in name order
    // first, so the reported dependency cannot be "whichever was created first".
    let mut asset_types = Vec::new();
    for (name, reference) in [
        ("SecondUsingType", "ns.SharedFormType"),
        ("FirstUsingType", "SharedFormType"),
    ] {
        let created = dz
            .call(
                "POST",
                &format!("/v2/domains/{domain}/asset-types"),
                Some(json!({
                    "name": name,
                    "owningProjectIdentifier": project,
                    "formsInput": { "shared": { "typeIdentifier": reference, "typeRevision": "1" } },
                })),
            )
            .await;
        assert_eq!(
            created.status, 201,
            "CreateAssetType {name}: expected 201, got {} {:?}",
            created.status, created.body
        );
        asset_types.push(created.s("name"));
    }
    let asset = dz
        .call(
            "POST",
            &format!("/v2/domains/{domain}/assets"),
            Some(json!({
                "name": "asset using the shared form type",
                "typeIdentifier": "FirstUsingType",
                "owningProjectIdentifier": project,
                "formsInput": [{
                    "formName": "shared",
                    "typeIdentifier": "SharedFormType",
                    "content": "{}",
                }],
            })),
        )
        .await;
    assert_eq!(
        asset.status, 201,
        "CreateAsset: expected 201, got {} {:?}",
        asset.status, asset.body
    );

    // The dependency reported is the asset type that LINKS the form type — the first in
    // name order — not the asset that merely carries a form of it, and it is the same
    // one on every call.
    let expected = deletion_prevented_message(
        &form_type_object(&domain, "SharedFormType"),
        &asset_object(
            &domain,
            asset_types
                .iter()
                .min()
                .expect("two asset types were created"),
        ),
    );
    for attempt in 1..=2 {
        let answer = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/form-types/SharedFormType"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (409, "ConflictException", expected.clone()),
            "attempt {attempt}: expected the same refusal naming the linking asset type, \
             got {} {:?}",
            answer.status,
            answer.body
        );
        assert!(
            !answer.s("message").contains(&asset.s("id")),
            "attempt {attempt}: an asset must not be reported while an asset type links \
             the form type, got {:?}",
            answer.s("message")
        );
    }
}

#[tokio::test]
async fn a_delete_the_service_cannot_satisfy_is_refused_with_a_modelled_error() {
    let dz = Emulator::new();
    let domain = dz.available_domain("refusal-delete-domain").await;
    let other_domain = dz.available_domain("refusal-delete-other").await;
    let project = dz.project(&domain, "refusal delete owner").await;
    form_type_with_revisions(&dz, &domain, &project, "ScopedForm", 1).await;

    // (label, domain in the path, form type in the path, status, wire code)
    let cases: Vec<(&str, String, String, u16, &str)> = vec![
        (
            // A permission is held ON a domain, so the caller holds none on an
            // identifier the store serves no domain for: authorization decides the
            // delete before any lookup.
            "a well-formed domain id that names no domain",
            "dzd_doesnotexist".to_string(),
            "ScopedForm".to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "a real domain and a form type nobody created",
            domain.clone(),
            "NoSuchForm".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The form type is stored in `domain`, so the OTHER domain does not hold it.
            "a form type that lives in another domain",
            other_domain.clone(),
            "ScopedForm".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            // `GetFormType` addresses a form type by the exact stored name, so a delete
            // does too: a qualified spelling of a stored name names no form type.
            "a qualified spelling of a stored name",
            domain.clone(),
            "some.namespace.ScopedForm".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a domain id outside the modelled pattern",
            "not-a-domain".to_string(),
            "ScopedForm".to_string(),
            400,
            "ValidationException",
        ),
        (
            "an identifier one character past the modelled maximum",
            domain.clone(),
            format!("F{}", "o".repeat(IDENTIFIER_MAX)),
            400,
            "ValidationException",
        ),
    ];

    for (label, path_domain, path_form_type, status, code) in cases {
        let answer = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{path_domain}/form-types/{path_form_type}"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (status, code),
            "case {label:?}: expected {status} {code}, got {} {:?} {:?}",
            answer.status,
            answer.error_code,
            answer.body
        );
        if status == 404 {
            // The message names the KIND that was not found and never the identifier
            // the caller supplied.
            let message = answer.s("message");
            assert!(
                message == DOMAIN_NOT_FOUND || message == FORM_TYPE_NOT_FOUND,
                "case {label:?}: expected a not-found message naming only the kind, got \
                 {message:?}"
            );
            assert!(
                !message.contains(&path_form_type) && !message.contains(&path_domain),
                "case {label:?}: a not-found message must not echo the request, got {message:?}"
            );
        }
    }

    // Every refusal above left the stored form type alone.
    let survivor = dz
        .call("GET", &format!("/v2/domains/{domain}/form-types/ScopedForm"), None)
        .await;
    assert_eq!(
        (survivor.status, survivor.s("revision").as_str()),
        (200, "1"),
        "a refused delete must store nothing, got {} {:?}",
        survivor.status,
        survivor.body
    );
}

#[tokio::test]
async fn a_form_type_in_a_domain_being_deleted_is_already_gone() {
    let dz = Emulator::new();
    let domain = dz.available_domain("deleting-domain").await;
    let project = dz.project(&domain, "deleting owner").await;
    form_type_with_revisions(&dz, &domain, &project, "DoomedForm", 1).await;

    let domain_deleted = dz.call("DELETE", &format!("/v2/domains/{domain}"), None).await;
    assert_eq!(
        domain_deleted.status, 202,
        "DeleteDomain: expected 202, got {} {:?}",
        domain_deleted.status, domain_deleted.body
    );

    // Straight away, while the domain is still reported DELETING, the form type is
    // already gone with the domain that held it; once the clock has taken the domain
    // away the identifier names no domain at all, so authorization denies the call.
    let cases: [(&str, bool, u16, &str, String); 2] = [
        (
            "while the domain is DELETING",
            false,
            404,
            "ResourceNotFoundException",
            DOMAIN_NOT_FOUND.to_string(),
        ),
        (
            "once it is gone",
            true,
            403,
            "AccessDeniedException",
            "User is not permitted to perform operation: DeleteFormType".to_string(),
        ),
    ];
    for (label, wait, status, code, message) in cases {
        if wait {
            tokio::time::sleep(SETTLED).await;
        }
        let answer = dz
            .call(
                "DELETE",
                &format!("/v2/domains/{domain}/form-types/DoomedForm"),
                None,
            )
            .await;
        assert_eq!(
            (answer.status, answer.error_code.as_str(), answer.s("message")),
            (status, code, message),
            "case {label:?}: expected the domain to be unreachable, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}
