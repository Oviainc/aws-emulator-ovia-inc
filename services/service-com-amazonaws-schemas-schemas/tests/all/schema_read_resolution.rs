// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the ONE resolution every call that names a resource
//! inside a registry makes before it answers.
//!
//! The rule: a call this service serves that names a registry, a schema in it or a
//! version of that schema resolves the path once, in that order — the store must hold a
//! registry of that name (else 404 `NotFoundException`, with the service's sentence for
//! a registry nothing created), that registry must hold a schema of that name (else 404,
//! with the sentence for a schema nothing created), and the schema must hold the version
//! the request names, or its latest stored version when the request names none (else
//! 404, with the sentence for a version the schema does not hold) — and otherwise the
//! call is served.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over:
//! the operation the request calls, and so the HTTP method, the route, how deep into the
//! path the call reaches (the registry, a schema, one version of a schema), whether the
//! version is named in the URI path, in a query parameter or not at all, and the status
//! a served call answers. Every row drives both its refusals and its success, so no row
//! can pass by refusing everything or by serving everything — and the version step is
//! driven with a label the schema never minted and with a label that is no version
//! number at all, so a row cannot pass by resolving the steps out of order either.
//! (`ExportSchema` makes the same resolution and is driven through it in
//! `tests/all/export_reads_the_stored_schema.rs`, where the schemas it can serve live.)
//!
//! Every case drives the real wire path: a signed `restJson1` request through
//! `MultiServiceRouter::dispatch`, exactly as the production router serves it. The HTTP
//! harness is the crate's one harness, imported from `registry_schema_lifecycle`, and
//! the `/v1/policy` route from `resource_policy`. Nothing here touches the state struct
//! directly, so a handler that refused without leaving the store alone would still have
//! to make the reads that follow agree.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_schemas_schemas::state::CODE_RESOURCE_NOT_FOUND;

use crate::registry_schema_lifecycle::{
    create_registry, listed_versions, registry_uri, schema_uri, versions_uri, Answer, Emulator,
};
use crate::resource_policy::policy_uri;

/// The registry every row below is served against.
const REGISTRY: &str = "read-resolution-registry";

/// The schema every row below is served against.
const SCHEMA: &str = "demo-schema";

/// A well-formed registry name no call in this suite ever creates.
const ABSENT_REGISTRY: &str = "registry-nobody-created";

/// A well-formed schema name no call in this suite ever creates.
const ABSENT_SCHEMA: &str = "schema-nobody-created";

/// The first version number this service mints for a schema, as a `SchemaVersion`
/// label on the wire.
const FIRST_VERSION: &str = "1";

/// `SchemaVersion` labels the seeded schema holds no version for: a number it never
/// minted, and a label that is no version number at all. Both name nothing, so both
/// take the version step's refusal.
const UNHELD_VERSIONS: [&str; 2] = ["7", "latest"];

/// The content the seeded schema is created with: an OpenAPI 3 document with component
/// schemas, so the export row has something to render as well.
fn schema_document() -> String {
    json!({
        "openapi": "3.0.0",
        "info": {"title": "read resolution", "version": "1.0.0"},
        "paths": {},
        "components": {"schemas": {"Event": {"type": "object"}}},
    })
    .to_string()
}

/// A policy document a `PutResourcePolicy` accepts.
const POLICY: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// The language the code-binding rows name: one of the languages this service
/// generates bindings for.
const LANGUAGE: &str = "Java8";

/// How deep into the registry → schema → version path a row's call reaches, and so
/// which steps of the one resolution it takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Depth {
    /// Names a registry only.
    Registry,
    /// Names a schema in a registry.
    Schema,
    /// Names a version of a schema in a registry.
    Version,
}

/// One call of this service that names a resource inside a registry: one row of the
/// table, and one of the handlers that used to carry its own copy of the resolution.
#[derive(Debug, Clone, Copy)]
enum Call {
    /// `DescribeRegistry`: the registry step alone.
    DescribeRegistry,
    /// `ListSchemas`: the registry step alone.
    ListSchemas,
    /// `SearchSchemas`: the registry step alone.
    SearchSchemas,
    /// `GetResourcePolicy`: the registry step alone, by the `registryName` query
    /// member.
    GetResourcePolicy,
    /// `ListSchemaVersions`: the registry and schema steps.
    ListSchemaVersions,
    /// `DescribeSchema`: all three steps, the version in a query member.
    DescribeSchema,
    /// `DescribeCodeBinding`: all three steps before the binding it reports.
    DescribeCodeBinding,
    /// `PutCodeBinding`: all three steps before the binding it attaches.
    PutCodeBinding,
    /// `DeleteSchemaVersion`: all three steps, the version in the URI path.
    DeleteSchemaVersion,
}

impl Call {
    /// Every row of the table.
    fn all() -> Vec<Self> {
        vec![
            Self::DescribeRegistry,
            Self::ListSchemas,
            Self::SearchSchemas,
            Self::GetResourcePolicy,
            Self::ListSchemaVersions,
            Self::DescribeSchema,
            Self::DescribeCodeBinding,
            Self::PutCodeBinding,
            Self::DeleteSchemaVersion,
        ]
    }

    /// The operation name, for the case label every assertion carries.
    fn case(self) -> &'static str {
        match self {
            Self::DescribeRegistry => "DescribeRegistry",
            Self::ListSchemas => "ListSchemas",
            Self::SearchSchemas => "SearchSchemas",
            Self::GetResourcePolicy => "GetResourcePolicy",
            Self::ListSchemaVersions => "ListSchemaVersions",
            Self::DescribeSchema => "DescribeSchema",
            Self::DescribeCodeBinding => "DescribeCodeBinding",
            Self::PutCodeBinding => "PutCodeBinding",
            Self::DeleteSchemaVersion => "DeleteSchemaVersion",
        }
    }

    /// How deep this row's call reaches.
    fn depth(self) -> Depth {
        match self {
            Self::DescribeRegistry
            | Self::ListSchemas
            | Self::SearchSchemas
            | Self::GetResourcePolicy => Depth::Registry,
            Self::ListSchemaVersions => Depth::Schema,
            Self::DescribeSchema
            | Self::DescribeCodeBinding
            | Self::PutCodeBinding
            | Self::DeleteSchemaVersion => Depth::Version,
        }
    }

    /// Whether the version is a REQUIRED part of this row's request, so the row cannot
    /// be driven with no version named.
    fn version_required(self) -> bool {
        matches!(self, Self::DeleteSchemaVersion)
    }

    /// How many versions this row's schema is seeded with.
    ///
    /// One is enough for every read. The DELETE needs two: a schema always holds at
    /// least one definition, so a delete of the only version a schema holds is refused
    /// as a conflict and would never reach the resolution this suite is about.
    fn seeded_versions(self) -> u32 {
        match self {
            Self::DeleteSchemaVersion => 2,
            _ => 1,
        }
    }

    /// The version labels this row's seed leaves the schema holding.
    fn seeded_version_labels(self) -> BTreeSet<String> {
        (1..=self.seeded_versions())
            .map(|version| version.to_string())
            .collect()
    }

    /// The status this row's call answers once the registry, schema and version it
    /// names are all there — the success status of the operation.
    fn served_status(self) -> u16 {
        match self {
            Self::DescribeRegistry
            | Self::ListSchemas
            | Self::SearchSchemas
            | Self::GetResourcePolicy
            | Self::ListSchemaVersions
            | Self::DescribeSchema
            | Self::DescribeCodeBinding => 200,
            // The put ACCEPTS the generation; it does not finish it.
            Self::PutCodeBinding => 202,
            Self::DeleteSchemaVersion => 204,
        }
    }

    /// Send this call against `registry`, `schema` and the `SchemaVersion` label
    /// `version` (where the row names one at all).
    async fn send(
        self,
        emulator: &Emulator,
        registry: &str,
        schema: &str,
        version: Option<&str>,
    ) -> Answer {
        let versioned = |base: String, separator: char| match version {
            Some(version) => format!("{base}{separator}schemaVersion={version}"),
            None => base,
        };
        match self {
            Self::DescribeRegistry => emulator.call("GET", &registry_uri(registry), None).await,
            Self::ListSchemas => {
                emulator
                    .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
                    .await
            }
            Self::SearchSchemas => {
                emulator
                    .call(
                        "GET",
                        &format!("{}/schemas/search?keywords=read", registry_uri(registry)),
                        None,
                    )
                    .await
            }
            Self::GetResourcePolicy => {
                emulator.call("GET", &policy_uri(Some(registry)), None).await
            }
            Self::ListSchemaVersions => {
                emulator
                    .call("GET", &versions_uri(registry, schema), None)
                    .await
            }
            Self::DescribeSchema => {
                emulator
                    .call("GET", &versioned(schema_uri(registry, schema), '?'), None)
                    .await
            }
            Self::DescribeCodeBinding => {
                emulator
                    .call(
                        "GET",
                        &versioned(
                            format!("{}/language/{LANGUAGE}", schema_uri(registry, schema)),
                            '?',
                        ),
                        None,
                    )
                    .await
            }
            Self::PutCodeBinding => {
                emulator
                    .call(
                        "POST",
                        &versioned(
                            format!("{}/language/{LANGUAGE}", schema_uri(registry, schema)),
                            '?',
                        ),
                        None,
                    )
                    .await
            }
            Self::DeleteSchemaVersion => {
                emulator
                    .call(
                        "DELETE",
                        &format!(
                            "{}/version/{}",
                            schema_uri(registry, schema),
                            version.unwrap_or(FIRST_VERSION)
                        ),
                        None,
                    )
                    .await
            }
        }
    }

    /// Seed [`REGISTRY`] with everything this row's call needs in order to be served:
    /// the registry, a schema with one version for every row that names a schema, the
    /// registry's policy for the policy read, and a started code binding for the
    /// binding read.
    async fn seed(self, emulator: &Emulator) {
        create_registry(emulator, REGISTRY).await;
        if self.depth() > Depth::Registry {
            create_schema(emulator, REGISTRY, SCHEMA).await;
            for round in 2..=self.seeded_versions() {
                let updated = emulator
                    .call(
                        "PUT",
                        &schema_uri(REGISTRY, SCHEMA),
                        Some(json!({"Content": schema_document(), "Type": "OpenApi3"})),
                    )
                    .await;
                assert_eq!(
                    updated.status,
                    200,
                    "case {:?}: seeding version {round} must succeed, got {} {:?}",
                    self.case(),
                    updated.status,
                    updated.body
                );
            }
        }
        match self {
            Self::GetResourcePolicy => {
                let attached = emulator
                    .call(
                        "PUT",
                        &policy_uri(Some(REGISTRY)),
                        Some(json!({"Policy": POLICY})),
                    )
                    .await;
                assert_eq!(
                    attached.status,
                    200,
                    "case {:?}: seeding a policy must succeed, got {} {:?}",
                    self.case(),
                    attached.status,
                    attached.body
                );
            }
            Self::DescribeCodeBinding => {
                let started = Self::PutCodeBinding
                    .send(emulator, REGISTRY, SCHEMA, None)
                    .await;
                assert_eq!(
                    started.status,
                    202,
                    "case {:?}: seeding a code binding must succeed, got {} {:?}",
                    self.case(),
                    started.status,
                    started.body
                );
            }
            _ => {}
        }
    }
}

/// Create a schema holding exactly one version, asserting it succeeded.
async fn create_schema(emulator: &Emulator, registry: &str, schema: &str) {
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": schema_document(), "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "seeding schema {schema:?} must succeed, got {} {:?}",
        created.status, created.body
    );
}

/// The service's one sentence for a registry nothing created.
fn no_such_registry_sentence(registry: &str) -> String {
    format!("Registry with name {registry} does not exist.")
}

/// The service's one sentence for a schema its registry does not hold.
fn no_such_schema_sentence(schema: &str) -> String {
    format!("Schema with name {schema} does not exist.")
}

/// The service's one sentence for a version the named schema does not hold: the
/// request's `SchemaName` and the label it named, and the sentence for the schema itself
/// where it named no version at all (a schema holding none is a resource no request can
/// produce, so no case below reaches that branch).
fn no_such_version_sentence(schema: &str, requested: Option<&str>) -> String {
    match requested {
        Some(version) => format!("Schema with name {schema} and version {version} does not exist."),
        None => no_such_schema_sentence(schema),
    }
}

/// Assert one answer is the 404 `NotFoundException` the resolution answers, worded with
/// `sentence`.
fn assert_not_found(answer: &Answer, label: &str, sentence: &str) {
    assert_eq!(
        answer.status, 404,
        "case {label:?}: expected 404 from the read resolution, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_type(),
        "NotFoundException",
        "case {label:?}: expected the NotFoundException shape, got {:?}",
        answer.header("x-amzn-errortype")
    );
    assert_eq!(
        answer.error_code(),
        "NotFoundException",
        "case {label:?}: expected the NotFoundException wire code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Code"),
        CODE_RESOURCE_NOT_FOUND,
        "case {label:?}: expected the {CODE_RESOURCE_NOT_FOUND} condition code, got {:?}",
        answer.body
    );
    assert_eq!(
        answer.str_at("Message"),
        sentence,
        "case {label:?}: wrong sentence for this step of the resolution, got {:?}",
        answer.body
    );
}

/// The version labels a schema still holds, as `ListSchemaVersions` reports them.
async fn held_versions(emulator: &Emulator, label: &str) -> BTreeSet<String> {
    let listed = emulator
        .call("GET", &versions_uri(REGISTRY, SCHEMA), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "case {label:?}: listing the schema's versions must succeed, got {} {:?}",
        listed.status, listed.body
    );
    listed_versions(&listed)
}

/// RULE: a call that names a registry, a schema in it or a version of that schema
/// resolves the path once and in that order — a registry this store does not hold is
/// refused 404 with the service's sentence for one nothing created, a schema its
/// registry does not hold with the sentence for a schema nothing created, and a version
/// the schema does not hold with the sentence for that, while a call whose whole path
/// resolves is served. Asserted over every operation of this service that makes the
/// resolution.
#[tokio::test]
async fn every_read_that_names_a_schema_resolves_the_path_the_same_way() {
    for call in Call::all() {
        let case = call.case();

        // Step 1: a name this store holds no registry for. Nothing is seeded, so the
        // very first step of the resolution is the one that refuses.
        let emulator = Emulator::new();
        let missing_registry = call
            .send(&emulator, ABSENT_REGISTRY, SCHEMA, Some(FIRST_VERSION))
            .await;
        assert_not_found(
            &missing_registry,
            &format!("{case}/absent-registry"),
            &no_such_registry_sentence(ABSENT_REGISTRY),
        );

        if call.depth() > Depth::Registry {
            // Step 2: the registry is there and holds a schema, but not the one this
            // request names — a later step's refusal must not be answered here.
            let emulator = Emulator::new();
            call.seed(&emulator).await;
            let missing_schema = call
                .send(&emulator, REGISTRY, ABSENT_SCHEMA, Some(FIRST_VERSION))
                .await;
            assert_not_found(
                &missing_schema,
                &format!("{case}/absent-schema"),
                &no_such_schema_sentence(ABSENT_SCHEMA),
            );
        }

        if call.depth() == Depth::Version {
            // Step 3: the registry and the schema are there; the version the request
            // names is not. Driven with a number the schema never minted and with a
            // label that is no version number at all.
            for unheld in UNHELD_VERSIONS {
                let emulator = Emulator::new();
                call.seed(&emulator).await;
                let label = format!("{case}/unheld-version-{unheld}");
                let missing_version = call
                    .send(&emulator, REGISTRY, SCHEMA, Some(unheld))
                    .await;
                assert_not_found(
                    &missing_version,
                    &label,
                    &no_such_version_sentence(SCHEMA, Some(unheld)),
                );
                // The refusal changed nothing: the schema still holds the versions it
                // was seeded with.
                let held = held_versions(&emulator, &label).await;
                assert_eq!(
                    held,
                    call.seeded_version_labels(),
                    "case {label:?}: the refused call must leave the schema's versions \
                     alone, got {held:?}"
                );
            }

            if !call.version_required() {
                // The same step, reached with NO version named: there is no such case to
                // reach, because a schema always holds a definition — the delete of the
                // only version a schema holds is refused rather than leaving one behind.
                let emulator = Emulator::new();
                call.seed(&emulator).await;
                let label = format!("{case}/schema-cannot-be-left-with-no-version");
                let refused = Call::DeleteSchemaVersion
                    .send(&emulator, REGISTRY, SCHEMA, Some(FIRST_VERSION))
                    .await;
                assert_eq!(
                    refused.status, 409,
                    "case {label:?}: deleting the only version must be refused, got {} {:?}",
                    refused.status, refused.body
                );
                let still_served = call.send(&emulator, REGISTRY, SCHEMA, None).await;
                assert_eq!(
                    still_served.status,
                    call.served_status(),
                    "case {label:?}: the schema still holds its definition, so the call is \
                     served; got {} {:?}",
                    still_served.status,
                    still_served.body
                );
            }
        }

        // And the same call against a path that resolves whole is SERVED: the
        // resolution refuses what is not there, not everything. Driven once with the
        // version left to the latest stored one and once with it named explicitly,
        // which is the other way the version step is reached.
        let requested: Vec<Option<&str>> = if call.depth() == Depth::Version {
            vec![Some(FIRST_VERSION), None]
        } else {
            vec![None]
        };
        for version in requested {
            if version.is_none() && call.version_required() {
                continue;
            }
            let emulator = Emulator::new();
            call.seed(&emulator).await;
            let served = call.send(&emulator, REGISTRY, SCHEMA, version).await;
            assert_eq!(
                served.status,
                call.served_status(),
                "case {case:?} (version {version:?}): expected {}, got {} {:?}",
                call.served_status(),
                served.status,
                served.body
            );
        }
    }
}
