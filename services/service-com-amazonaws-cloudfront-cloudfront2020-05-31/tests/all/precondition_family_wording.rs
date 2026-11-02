// SPDX-License-Identifier: CC0-1.0

//! How this service words a mutation it refuses over the `If-Match` version the request
//! carried, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. One rule, asserted over every API family that evaluates an `If-Match`:
//!
//! * a header that carries no version token at all is "missing or not valid"
//!   (`InvalidIfMatchVersion`, HTTP 400) for every one of them;
//! * a header that carries a WELL-FORMED token the addressed resource does not hold is the
//!   failed precondition (`PreconditionFailed`, HTTP 412), worded with the fixed sentence the
//!   operation's API FAMILY states for it — the distribution-tenant surface (the tenants, the
//!   connection groups and connection functions that route them, and the web-ACL associations
//!   of both) reports the request as missing a requirement, the policy surface (the reusable
//!   configuration objects a distribution references by id: the cache policies, the origin
//!   request policies and the key groups) reports the precondition of the operation, and the
//!   resource-DOCUMENT surface (a distribution, an identity, a function, a key value store, a
//!   field-level encryption record) reports it of the request's header fields;
//! * `DeleteFunction` is the one operation that reads "missing or not valid" as "not valid
//!   FOR THIS FUNCTION", so a well-formed token that is not the function's current one is
//!   `InvalidIfMatchVersion` (400) there instead;
//! * every one of those sentences is FIXED text: it carries neither the token the request
//!   sent nor the resource it addressed, so two callers whose only difference is the token
//!   they held read the same sentence;
//! * and the refusal changes nothing: the mutation lands once it carries the version the
//!   resource is really at.
//!
//! The rows differ along what the rule generalises over: the API family (three of them), the
//! operation within it (eight, over seven resource kinds, split between deletes and updates
//! and between a required and an optional `IfMatch` member), and the FORM the token takes (no
//! version token at all, a well-formed token nothing holds, and that same token quoted as a
//! strong and as a weak entity tag — HTTP's three spellings of one value).
//!
//! The expected sentences are LITERALS below rather than the crate's own constants, so a
//! wrong constant cannot make this suite pass. The harness (the router, the request helpers
//! and each kind's configuration table) is the one the per-resource suites use, reused rather
//! than copied.

use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{create_policy as create_cache_policy, policy_table};
use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_delete::set_enabled;
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::function_lifecycle::{base64, create_function, create_table as function_table};
use crate::key_group_lifecycle::{create_group, mint_public_keys, Group};
use crate::key_value_store_lifecycle::{create_key_value_store, create_table as kvs_table};
use crate::origin_access_identity_lifecycle::{create_identity, config_table as identity_table};

// ---------------------------------------------------------------------------
// The sentences the service serves, as literals
// ---------------------------------------------------------------------------

/// `InvalidIfMatchVersion`'s sentence: a property of the HEADER the request carried, so one
/// sentence answers every operation that evaluates one.
const INVALID_IF_MATCH_VERSION: &str = "The If-Match version is missing or not valid for the resource.";

/// `PreconditionFailed` on the distribution-tenant surface.
const MISSING_REQUIREMENTS: &str =
    "Your request is missing requirements for this operation. Update your request and try again.";

/// `PreconditionFailed` on the policy surface.
const PRECONDITIONS_FOR_THIS_OPERATION: &str =
    "The request failed because it didn't meet the preconditions for this operation.";

/// `PreconditionFailed` on the resource-document surface.
const PRECONDITIONS_IN_HEADER_FIELDS: &str =
    "The request failed because it didn't meet the preconditions in one or more \
     request-header fields.";

/// A version token that is well formed — `E` + 13 characters of the shape this service mints
/// — but that nothing in a fresh emulator holds, so a precondition carrying it is STALE
/// rather than unreadable.
const UNHELD_VERSION_TOKEN: &str = "E1ABCDEFGHIJKL";

// ---------------------------------------------------------------------------
// The families and the operations in them
// ---------------------------------------------------------------------------

/// Which API family an operation belongs to, which is what decides the sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Family {
    /// The distribution tenants, the connection groups and functions that route them, and
    /// the web-ACL associations of both.
    DistributionTenant,
    /// The reusable configuration objects a distribution references by id.
    Policy,
    /// The resource DOCUMENTS a caller conditionally rewrites.
    ResourceDocument,
}

impl Family {
    /// The sentence this family states a failed precondition with.
    fn sentence(self) -> &'static str {
        match self {
            Self::DistributionTenant => MISSING_REQUIREMENTS,
            Self::Policy => PRECONDITIONS_FOR_THIS_OPERATION,
            Self::ResourceDocument => PRECONDITIONS_IN_HEADER_FIELDS,
        }
    }
}

/// One operation under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    DeleteDistributionTenant,
    UpdateConnectionGroup,
    DeleteKeyValueStore,
    DeleteCachePolicy,
    UpdateKeyGroup,
    UpdateFunction,
    DeleteFunction,
    UpdateCloudFrontOriginAccessIdentity,
}

/// Every operation, with the family that words its failed precondition.
const OPS: &[(Op, Family)] = &[
    (Op::DeleteDistributionTenant, Family::DistributionTenant),
    (Op::UpdateConnectionGroup, Family::DistributionTenant),
    (Op::DeleteKeyValueStore, Family::ResourceDocument),
    (Op::DeleteCachePolicy, Family::Policy),
    (Op::UpdateKeyGroup, Family::Policy),
    (Op::UpdateFunction, Family::ResourceDocument),
    (Op::DeleteFunction, Family::ResourceDocument),
    (
        Op::UpdateCloudFrontOriginAccessIdentity,
        Family::ResourceDocument,
    ),
];

impl Op {
    /// What a WELL-FORMED token this resource does not hold is refused with: the family's
    /// failed precondition, except for the one operation that classifies it as a version that
    /// is not valid for the resource.
    fn stale(self, family: Family) -> (u16, &'static str, &'static str) {
        match self {
            Self::DeleteFunction => (400, "InvalidIfMatchVersion", INVALID_IF_MATCH_VERSION),
            _ => (412, "PreconditionFailed", family.sentence()),
        }
    }
}

// ---------------------------------------------------------------------------
// The mutation each row drives
// ---------------------------------------------------------------------------

/// One prepared mutation: the request it is, and the version token the resource it addresses
/// is really at.
struct Prepared {
    method: &'static str,
    uri: String,
    body: Option<String>,
    /// The version the resource stands at, so the row can show the mutation lands with it.
    etag: String,
    /// The status the mutation answers when it carries that version.
    served: u16,
}

/// Stand up one resource of the kind `op` addresses and describe the mutation against it.
async fn prepare(router: &MultiServiceRouter, op: Op, tag: &str) -> Prepared {
    match op {
        Op::DeleteDistributionTenant => {
            let distribution = multi_tenant_distribution(router, &format!("ref-{tag}")).await;
            let body = format!(
                "<CreateDistributionTenantRequest \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <DistributionId>{}</DistributionId><Name>{tag}</Name>\
                 <Domains><member><Domain>{tag}.example.com</Domain></member></Domains>\
                 <Parameters><member><Name>tenantName</Name><Value>{tag}</Value></member>\
                 </Parameters>{}</CreateDistributionTenantRequest>",
                escape(&distribution),
                managed_certificate_request()
            );
            let created = create_distribution_tenant(router, &body).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the tenant must be created, got {} with {}",
                created.status, created.raw
            );
            let id = minted_id(&created);
            // A tenant is deleted only once it is disabled, so the row's own version token is
            // the one that disabling handed back.
            let disabled = set_enabled(router, &id, false, created.header("etag")).await;
            assert_eq!(
                disabled.status, 200,
                "case {tag}: the tenant must be disabled before the delete, got {} with {}",
                disabled.status, disabled.raw
            );
            Prepared {
                method: "DELETE",
                uri: format!("/{API_VERSION}/distribution-tenant/{id}"),
                body: None,
                etag: disabled.header("etag").to_string(),
                served: 204,
            }
        }
        Op::UpdateConnectionGroup => {
            let body = format!(
                "<CreateConnectionGroupRequest \
                 xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                 <Name>{tag}</Name></CreateConnectionGroupRequest>"
            );
            let created = create_connection_group(router, &body).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the connection group must be created, got {} with {}",
                created.status, created.raw
            );
            Prepared {
                method: "PUT",
                uri: format!("/{API_VERSION}/connection-group/{}", minted_id(&created)),
                body: Some(format!(
                    "<UpdateConnectionGroupRequest \
                     xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                     <Enabled>false</Enabled></UpdateConnectionGroupRequest>"
                )),
                etag: created.header("etag").to_string(),
                served: 200,
            }
        }
        Op::DeleteKeyValueStore => {
            let mut row = kvs_table()
                .into_iter()
                .next()
                .expect("the key value store table has a row");
            row.name = tag.replace('-', "_");
            let created = create_key_value_store(router, &row.xml()).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the key value store must be created, got {} with {}",
                created.status, created.raw
            );
            Prepared {
                method: "DELETE",
                uri: format!("/{API_VERSION}/key-value-store/{}", row.name),
                body: None,
                etag: created.header("etag").to_string(),
                served: 204,
            }
        }
        Op::DeleteCachePolicy => {
            let mut row = policy_table()
                .into_iter()
                .next()
                .expect("the cache policy table has a row");
            row.name = tag.to_string();
            let created = create_cache_policy(router, &row.xml()).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the cache policy must be created, got {} with {}",
                created.status, created.raw
            );
            Prepared {
                method: "DELETE",
                uri: format!("/{API_VERSION}/cache-policy/{}", minted_id(&created)),
                body: None,
                etag: created.header("etag").to_string(),
                served: 204,
            }
        }
        Op::UpdateKeyGroup => {
            let keys = mint_public_keys(router, tag, 1).await;
            let group = Group {
                case: tag.to_string(),
                name: tag.to_string(),
                items: keys,
                comment: Some(format!("the key group of {tag}")),
            };
            let created = create_group(router, &group.xml()).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the key group must be created, got {} with {}",
                created.status, created.raw
            );
            let updated = Group {
                comment: Some(format!("the key group of {tag}, updated")),
                ..group
            };
            Prepared {
                method: "PUT",
                uri: format!("/{API_VERSION}/key-group/{}", minted_id(&created)),
                body: Some(updated.xml()),
                etag: created.header("etag").to_string(),
                served: 200,
            }
        }
        Op::UpdateFunction | Op::DeleteFunction => {
            let store = crate::function_lifecycle::create_key_value_store(
                router,
                &format!("{}_store", tag.replace('-', "_")),
            )
            .await;
            let mut row = function_table(&store)
                .into_iter()
                .next()
                .expect("the function table has a row");
            row.name = tag.replace('-', "_");
            let created = create_function(router, &row.xml()).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the function must be created, got {} with {}",
                created.status, created.raw
            );
            let etag = created.header("etag").to_string();
            if op == Op::DeleteFunction {
                return Prepared {
                    method: "DELETE",
                    uri: format!("/{API_VERSION}/function/{}", row.name),
                    body: None,
                    etag,
                    served: 204,
                };
            }
            Prepared {
                method: "PUT",
                uri: format!("/{API_VERSION}/function/{}", row.name),
                body: Some(format!(
                    "<UpdateFunctionRequest \
                     xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
                     {}<FunctionCode>{}</FunctionCode></UpdateFunctionRequest>",
                    row.config_xml(),
                    base64(&row.code)
                )),
                etag,
                served: 200,
            }
        }
        Op::UpdateCloudFrontOriginAccessIdentity => {
            let mut config = identity_table()
                .into_iter()
                .next()
                .expect("the identity table has a row");
            config.caller_reference = tag.to_string();
            let created = create_identity(router, &config.xml()).await;
            assert_eq!(
                created.status, 201,
                "case {tag}: the origin access identity must be created, got {} with {}",
                created.status, created.raw
            );
            Prepared {
                method: "PUT",
                uri: format!(
                    "/{API_VERSION}/origin-access-identity/cloudfront/{}/config",
                    minted_id(&created)
                ),
                body: Some(config.xml()),
                etag: created.header("etag").to_string(),
                served: 200,
            }
        }
    }
}

/// The identifier a create minted.
fn minted_id(answer: &Answer) -> String {
    answer
        .root()
        .find_text("Id")
        .unwrap_or_default()
        .to_string()
}

/// The message one refusal carries, or `""`.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// The `If-Match` values every row is driven over: what each is, the value, and whether it is
/// a version token at all.
fn tokens() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "a value that is no version token at all",
            "not-a-version".to_string(),
            false,
        ),
        (
            "a well-formed token nothing holds",
            UNHELD_VERSION_TOKEN.to_string(),
            true,
        ),
        (
            "that token quoted as an entity tag",
            format!("\"{UNHELD_VERSION_TOKEN}\""),
            true,
        ),
        (
            "that token as a weak entity tag",
            format!("W/\"{UNHELD_VERSION_TOKEN}\""),
            true,
        ),
    ]
}

// ---------------------------------------------------------------------------
// The rule
// ---------------------------------------------------------------------------

/// A mutation whose `If-Match` does not carry the version the addressed resource stands at is
/// refused with the status, the code and the FIXED sentence its API family states for that
/// condition — and the same mutation lands as soon as it carries that version.
#[tokio::test]
async fn a_failed_precondition_is_worded_by_the_operations_api_family() {
    for (op, family) in OPS {
        // One emulator per operation, so a row that removes the resource it addresses cannot
        // change what the next one reads.
        let router = emulator();
        let tag = format!("precondition-{}", OPS.iter().position(|(o, _)| o == op).unwrap_or(0));
        let prepared = prepare(&router, *op, &tag).await;

        let mut sentences: Vec<String> = Vec::new();
        for (form, token, is_version) in tokens() {
            let case = format!("{op:?} with {form}");
            let answer = call(
                &router,
                prepared.method,
                &prepared.uri,
                prepared.body.as_deref(),
                Some(&token),
            )
            .await;
            let (status, code, sentence) = if is_version {
                op.stale(*family)
            } else {
                (400, "InvalidIfMatchVersion", INVALID_IF_MATCH_VERSION)
            };
            assert_eq!(
                answer.status, status,
                "case {case}: expected HTTP {status}, got {} with {}",
                answer.status, answer.raw
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case}: expected {code}, got {:?} in {}",
                answer.error_code(),
                answer.raw
            );
            let actual = message(&answer);
            assert_eq!(
                actual, sentence,
                "case {case}: expected the sentence {sentence:?}, got {actual:?}"
            );
            // Nothing of the request reaches the sentence: neither the token it sent nor the
            // resource it addressed.
            assert!(
                !actual.contains(UNHELD_VERSION_TOKEN),
                "case {case}: the refusal must not carry the token the request sent, got \
                 {actual:?}"
            );
            if is_version {
                sentences.push(actual.to_string());
            }
        }

        // One sentence for every spelling of the one stale token.
        assert!(
            sentences.windows(2).all(|pair| pair[0] == pair[1]),
            "case {op:?}: expected one sentence for every token form, got {sentences:?}"
        );

        // The refused mutations changed nothing: the resource still stands at the version its
        // create (or the disable before it) handed back, so the mutation lands with it.
        let served = call(
            &router,
            prepared.method,
            &prepared.uri,
            prepared.body.as_deref(),
            Some(&prepared.etag),
        )
        .await;
        assert_eq!(
            served.status, prepared.served,
            "case {op:?}: expected HTTP {} once the request carries the current version, got {} \
             with {}",
            prepared.served, served.status, served.raw
        );
    }
}
