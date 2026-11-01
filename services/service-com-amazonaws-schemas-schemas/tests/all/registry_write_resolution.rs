// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE resolution every write that names a registry
//! makes before it changes anything.
//!
//! The rule: a write this service serves that names a registry resolves it once — the
//! store must hold a registry of that name (else 404 `NotFoundException`, with the
//! service's sentence for a registry nothing created), and what is in a service-owned
//! registry is AWS's, so a caller may read it but not write to it (else 403
//! `ForbiddenException`, with the service's sentence for that refusal) — and otherwise
//! the write goes through.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the operation the request calls, and so the HTTP method, the route, the resource
//! inside the registry the write would reach (the registry itself, a schema, one
//! version of a schema, the registry's resource-based policy) and the status a served
//! write answers. Every row drives all three outcomes, so no row can pass by refusing
//! everything or by serving everything.
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, and
//! the `/v1/policy` route from `resource_policy`. Nothing here touches the state struct
//! directly, so a handler that refused without leaving the store alone would still have
//! to make the reads that follow agree.

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::{
    AWS_MANAGED_REGISTRIES, CODE_FORBIDDEN, CODE_RESOURCE_NOT_FOUND,
};

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_names, schema_uri, Answer, Emulator,
};
use crate::resource_policy::policy_uri;

/// The schema a row's write names inside the registry it targets.
const SCHEMA: &str = "demo-schema";

/// The first version number this service mints for a schema, as a `SchemaVersion`
/// label on the wire.
const FIRST_VERSION: &str = "1";

/// A well-formed registry name no call in this suite ever creates.
const ABSENT_REGISTRY: &str = "registry-nobody-created";

/// A policy document a `PutResourcePolicy` accepts.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// One write of this service that names a registry: one row of the table, and one of
/// the handlers that used to carry its own copy of the resolve-then-refuse step.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// `CreateSchema`: add a schema to the registry.
    CreateSchema,
    /// `UpdateSchema`: write a new version of a schema in the registry.
    UpdateSchema,
    /// `DeleteSchema`: remove a schema from the registry.
    DeleteSchema,
    /// `DeleteSchemaVersion`: remove one version of a schema in the registry.
    DeleteSchemaVersion,
    /// `UpdateRegistry`: rewrite the registry's own description.
    UpdateRegistry,
    /// `DeleteRegistry`: remove the registry.
    DeleteRegistry,
    /// `PutResourcePolicy`: attach a resource-based policy to the registry.
    PutResourcePolicy,
    /// `DeleteResourcePolicy`: remove the registry's resource-based policy.
    DeleteResourcePolicy,
}

impl Write {
    /// Every row of the table.
    fn all() -> Vec<Self> {
        vec![
            Self::CreateSchema,
            Self::UpdateSchema,
            Self::DeleteSchema,
            Self::DeleteSchemaVersion,
            Self::UpdateRegistry,
            Self::DeleteRegistry,
            Self::PutResourcePolicy,
            Self::DeleteResourcePolicy,
        ]
    }

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::CreateSchema => "CreateSchema",
            Self::UpdateSchema => "UpdateSchema",
            Self::DeleteSchema => "DeleteSchema",
            Self::DeleteSchemaVersion => "DeleteSchemaVersion",
            Self::UpdateRegistry => "UpdateRegistry",
            Self::DeleteRegistry => "DeleteRegistry",
            Self::PutResourcePolicy => "PutResourcePolicy",
            Self::DeleteResourcePolicy => "DeleteResourcePolicy",
        }
    }

    /// The status this write answers once the registry it names is one the caller owns
    /// and holds what the write needs — the success status of the operation.
    fn served_status(self) -> u16 {
        match self {
            Self::CreateSchema => 201,
            Self::UpdateSchema
            | Self::UpdateRegistry
            | Self::PutResourcePolicy
            // Every method on the policy endpoint answers 200, this removal included.
            | Self::DeleteResourcePolicy => 200,
            Self::DeleteSchema | Self::DeleteSchemaVersion | Self::DeleteRegistry => 204,
        }
    }

    /// Send this write against `registry`.
    async fn send(self, emulator: &Emulator, registry: &str) -> Answer {
        let schema_document = json!({
            "openapi": "3.0.0",
            "info": {"title": "written by a caller", "version": "1.0.0"},
            "paths": {},
        })
        .to_string();
        match self {
            Self::CreateSchema => {
                emulator
                    .call(
                        "POST",
                        &schema_uri(registry, SCHEMA),
                        Some(json!({"Content": schema_document, "Type": "OpenApi3"})),
                    )
                    .await
            }
            Self::UpdateSchema => {
                emulator
                    .call(
                        "PUT",
                        &schema_uri(registry, SCHEMA),
                        Some(json!({"Content": schema_document, "Type": "OpenApi3"})),
                    )
                    .await
            }
            Self::DeleteSchema => {
                emulator
                    .call("DELETE", &schema_uri(registry, SCHEMA), None)
                    .await
            }
            Self::DeleteSchemaVersion => {
                emulator
                    .call(
                        "DELETE",
                        &format!("{}/version/{FIRST_VERSION}", schema_uri(registry, SCHEMA)),
                        None,
                    )
                    .await
            }
            Self::UpdateRegistry => {
                emulator
                    .call(
                        "PUT",
                        &registry_uri(registry),
                        Some(json!({"Description": "rewritten by a caller"})),
                    )
                    .await
            }
            Self::DeleteRegistry => emulator.call("DELETE", &registry_uri(registry), None).await,
            Self::PutResourcePolicy => {
                emulator
                    .call(
                        "PUT",
                        &policy_uri(Some(registry)),
                        Some(json!({"Policy": POLICY})),
                    )
                    .await
            }
            Self::DeleteResourcePolicy => {
                emulator
                    .call("DELETE", &policy_uri(Some(registry)), None)
                    .await
            }
        }
    }

    /// Create `registry` as a registry THIS caller owns, holding what this row's write
    /// needs in order to be served: nothing but the registry for a create, a schema for
    /// a write that names one, a policy for a policy removal, and an empty registry for
    /// the registry delete.
    async fn seed_owned(self, emulator: &Emulator, registry: &str) {
        create_registry(emulator, registry).await;
        match self {
            // A create needs only the registry; so does the registry's own update, the
            // delete of an empty registry, and the first write of its policy.
            Self::CreateSchema
            | Self::UpdateRegistry
            | Self::DeleteRegistry
            | Self::PutResourcePolicy => {}
            Self::UpdateSchema | Self::DeleteSchema | Self::DeleteSchemaVersion => {
                let created = Self::CreateSchema.send(emulator, registry).await;
                assert_eq!(
                    created.status,
                    201,
                    "case {:?}: seeding a schema must succeed, got {} {:?}",
                    self.case(),
                    created.status,
                    created.body
                );
                // The version delete needs a SECOND definition: a schema always holds
                // at least one, so removing the only version a schema has is refused as
                // a conflict and would never reach the registry resolution this suite is
                // about.
                if matches!(self, Self::DeleteSchemaVersion) {
                    let updated = Self::UpdateSchema.send(emulator, registry).await;
                    assert_eq!(
                        updated.status,
                        200,
                        "case {:?}: seeding a second version must succeed, got {} {:?}",
                        self.case(),
                        updated.status,
                        updated.body
                    );
                }
            }
            Self::DeleteResourcePolicy => {
                let attached = Self::PutResourcePolicy.send(emulator, registry).await;
                assert_eq!(
                    attached.status,
                    200,
                    "case {:?}: seeding a policy must succeed, got {} {:?}",
                    self.case(),
                    attached.status,
                    attached.body
                );
            }
        }
    }
}

/// The service's one sentence for a caller's write to a service-owned registry, as
/// every handler that refuses one words it.
///
/// Shared with [`crate::code_bindings_for_managed_schemas`], which asserts the same
/// refusal on the schema a code binding was just generated from.
pub(crate) fn service_owned_sentence(registry: &str) -> String {
    format!(
        "Registry {registry} is an AWS-managed registry; its contents are owned by AWS \
         and cannot be created, updated or deleted."
    )
}

/// The service's one sentence for a registry nothing created.
fn no_such_registry_sentence(registry: &str) -> String {
    format!("Registry with name {registry} does not exist.")
}

/// The schema names a registry reports through `ListSchemas`.
async fn listed_schemas(emulator: &Emulator, registry: &str) -> Vec<String> {
    let listed = emulator
        .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "listing the schemas of {registry:?} must succeed, got {} {:?}",
        listed.status, listed.body
    );
    schema_names(&listed)
}

/// RULE: a write that names a registry resolves it once — a name this store holds no
/// registry for is refused 404 `NotFoundException`, a service-owned registry is refused
/// 403 `ForbiddenException` with the service's one sentence for that refusal, and a
/// registry the caller owns is written to. Asserted over every write this service
/// serves that names a registry.
#[tokio::test]
async fn every_write_that_names_a_registry_resolves_it_the_same_way() {
    for write in Write::all() {
        let case = write.case();

        // Source: `crate::state::AWS_MANAGED_REGISTRIES` — the service-owned registries
        // every account sees before any call is made.
        for registry in AWS_MANAGED_REGISTRIES {
            let emulator = Emulator::new();
            let label = format!("{case}/{registry}");
            let before = listed_schemas(&emulator, registry).await;

            let refused = write.send(&emulator, registry).await;
            assert_eq!(
                refused.status, 403,
                "case {label:?}: expected 403 from a write to a service-owned registry, \
                 got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type(),
                "ForbiddenException",
                "case {label:?}: expected the ForbiddenException shape, got {:?}",
                refused.header("x-amzn-errortype")
            );
            assert_eq!(
                refused.error_code(),
                "ForbiddenException",
                "case {label:?}: expected the ForbiddenException wire code, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Code"),
                CODE_FORBIDDEN,
                "case {label:?}: expected the {CODE_FORBIDDEN} condition code, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.str_at("Message"),
                service_owned_sentence(registry),
                "case {label:?}: wrong sentence for a write to a service-owned registry, \
                 got {:?}",
                refused.body
            );

            // The refusal changed nothing: the registry is still readable and still
            // holds exactly the schemas the store seeded into it.
            let described = emulator.call("GET", &registry_uri(registry), None).await;
            assert_eq!(
                described.status, 200,
                "case {label:?}: the service-owned registry must survive the refusal, \
                 got {} {:?}",
                described.status, described.body
            );
            let after = listed_schemas(&emulator, registry).await;
            assert_eq!(
                after, before,
                "case {label:?}: the refused write must leave the registry's schemas \
                 alone, got {after:?}"
            );
        }

        // A name this store holds no registry for: the other refusal the one resolution
        // answers, on the same route and with the same request.
        let emulator = Emulator::new();
        let missing = write.send(&emulator, ABSENT_REGISTRY).await;
        assert_eq!(
            missing.status, 404,
            "case {case:?}: expected 404 for a registry nothing created, got {} {:?}",
            missing.status, missing.body
        );
        assert_eq!(
            missing.error_type(),
            "NotFoundException",
            "case {case:?}: expected the NotFoundException shape, got {:?}",
            missing.header("x-amzn-errortype")
        );
        assert_eq!(
            missing.str_at("Code"),
            CODE_RESOURCE_NOT_FOUND,
            "case {case:?}: expected the {CODE_RESOURCE_NOT_FOUND} condition code, got {:?}",
            missing.body
        );
        assert_eq!(
            missing.str_at("Message"),
            no_such_registry_sentence(ABSENT_REGISTRY),
            "case {case:?}: wrong sentence for a registry nothing created, got {:?}",
            missing.body
        );

        // And the same write against a registry this caller created is SERVED: the
        // resolution refuses who may not write, not everyone.
        let emulator = Emulator::new();
        let own = "caller-owned-registry";
        write.seed_owned(&emulator, own).await;
        let served = write.send(&emulator, own).await;
        assert_eq!(
            served.status,
            write.served_status(),
            "case {case:?}: expected {} from the same write to a registry the caller \
             owns, got {} {:?}",
            write.served_status(),
            served.status,
            served.body
        );
    }
}
