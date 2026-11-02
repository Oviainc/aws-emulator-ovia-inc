// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE every already-exists conflict of this service carries, end to end over HTTP.
//!
//! Every assertion goes through the router, so each case exercises routing, restXml
//! (de)serialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rule under test, over every resource kind of this service whose create
//! refuses a name the account already holds:
//!
//! * the conflict is HTTP 409 under the error code the model binds for that resource kind,
//!   and its message is the FIXED sentence the service states for that code — the resource
//!   kind and the condition, with nothing taken from the request or from the store in it;
//! * so the refusal carries neither the name submitted nor the identifier of the resource
//!   that holds it, and two callers whose only difference is the name they chose read the
//!   same sentence;
//! * the same rule holds at the OTHER entry point into the uniqueness rule — the update
//!   that moves a name a sibling resource already holds — which answers the one sentence
//!   its create answers;
//! * and the refusal changes nothing: the resource that holds the name is readable
//!   afterwards with the identity its own create minted.
//!
//! The rows span what the rule generalises over: the resource KIND (eleven of them, seven
//! error codes between them), and for each kind the values a caller may choose — the ends of
//! what the name members admit (the shortest form every kind takes and the 64-character
//! bound the strictest of them publishes) and the character classes they admit (hyphens,
//! underscores, digits, mixed case). Each row's repeat carries a configuration that DIFFERS
//! from the stored one, so a kind that replaced the resource rather than refusing the value
//! would show up in the read-back beside the assertion.
//!
//! The second suite below covers the kinds the first one's harness cannot drive: the two
//! addressed by NAME rather than by a minted `Id`, and the one whose conflict is over the
//! `CallerReference` rather than the name.
//!
//! The expected sentences are LITERALS in the tables below rather than the crate's own
//! constants, so a wrong constant cannot make this suite pass — including the two conflicts
//! the service answers with NO message at all.
//!
//! The harness (the router, the request helpers and each kind's configuration table) is the
//! one the per-resource suites use, reused rather than copied; only the two request
//! documents whose builders those suites keep private (a connection group's, and a tenant's
//! — whose name and claimed domain have to vary independently here) are built in this file.

use smithy_server_runtime::MultiServiceRouter;

use crate::cache_policy_lifecycle::{
    create_policy as create_cache_policy, policy_table as cache_policy_table,
    Policy as CachePolicy,
};
use crate::cache_policy_update::update_policy as update_cache_policy;
use crate::connection_function_lifecycle::{
    create_connection_function, create_table as connection_function_table,
    describe_connection_function, key_value_store_arn, Create as ConnectionFunction,
};
use crate::connection_groups::create_connection_group;
use crate::distribution_lifecycle::{call, emulator, Answer, API_VERSION};
use crate::distribution_tenant_lifecycle::{
    create_distribution_tenant, escape, managed_certificate_request, multi_tenant_distribution,
};
use crate::function_lifecycle::{
    create_function, create_key_value_store as function_key_value_store,
    create_table as function_create_table, describe_function,
};
use crate::key_group_lifecycle::{create_group, mint_public_keys, Group};
use crate::key_value_store_lifecycle::{
    create_key_value_store, create_table as key_value_store_table, describe_key_value_store,
};
use crate::origin_access_control_lifecycle::{create_control, table as control_table};
use crate::origin_request_policy_lifecycle::{
    create_policy as create_origin_request_policy, policy_table as origin_request_policy_table,
    Policy as OriginRequestPolicy,
};
use crate::public_key_lifecycle::{create_key, get_key, pem, Key as PublicKey};
use crate::response_headers_policy_lifecycle::{
    create_policy as create_response_headers_policy,
    policy_table as response_headers_policy_table, Policy as ResponseHeadersPolicy,
};

// ---------------------------------------------------------------------------
// The kinds under test
// ---------------------------------------------------------------------------

/// One resource kind whose create refuses a name the account already holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    ConnectionGroup,
    KeyGroup,
    OriginAccessControl,
    ResponseHeadersPolicy,
    CachePolicy,
    ConnectionFunction,
    OriginRequestPolicy,
    DistributionTenant,
}

/// Every kind under test, with the error code the model binds for its conflict and the
/// sentence the service states for that code.
const KINDS: &[(Kind, &str, &str)] = &[
    (
        Kind::ConnectionGroup,
        "EntityAlreadyExists",
        "A connection group with the same name already exists in this AWS account.",
    ),
    (
        Kind::KeyGroup,
        "KeyGroupAlreadyExists",
        "Another key group with the same name already exists within the aws account.",
    ),
    (
        Kind::OriginAccessControl,
        "OriginAccessControlAlreadyExists",
        "An origin access control with the same name already exists.",
    ),
    (
        Kind::ResponseHeadersPolicy,
        "ResponseHeadersPolicyAlreadyExists",
        "Another response headers policy with the same name already exists within the aws \
         account.",
    ),
    (
        Kind::CachePolicy,
        "CachePolicyAlreadyExists",
        "Another cache policy with the same name already exists within the aws account.",
    ),
    (
        Kind::ConnectionFunction,
        "EntityAlreadyExists",
        "A connection function with the same name already exists in this AWS account.",
    ),
    (
        Kind::OriginRequestPolicy,
        "OriginRequestPolicyAlreadyExists",
        "Another origin request policy with the same name already exists within the aws \
         account.",
    ),
    (
        Kind::DistributionTenant,
        "EntityAlreadyExists",
        "A distribution tenant with the same name already exists in this AWS account.",
    ),
];

/// The kinds whose UPDATE takes the same name member, so the uniqueness rule has a second
/// entry point: the update that moves onto this resource a name a sibling already holds.
///
/// `UpdateOriginAccessControl` reaches the same rule, but its conflict is decided over the
/// whole parameter set rather than the name, so it is asserted where that comparison is
/// already driven — `crate::origin_access_control_update`'s
/// `parameters_another_control_holds_cannot_be_moved_onto_this_one` — rather than here.
const UPDATED_KINDS: &[Kind] = &[Kind::KeyGroup, Kind::CachePolicy, Kind::OriginRequestPolicy];

/// The names every kind is driven over.
///
/// `CreateConnectionFunction$Name` is `length 1..64` under `^[a-zA-Z0-9-_]{1,64}$` and
/// `CreateDistributionTenant$Name` is `^[a-zA-Z0-9][a-zA-Z0-9-.]{1,126}[a-zA-Z0-9]$`
/// (`smithy_query(query="operation",operation="CreateConnectionFunction,CreateDistributionTenant")`),
/// so these rows sit inside what EVERY kind's name member admits: the shortest form all of
/// them take, the 64-character bound the strictest of them publishes, and the character
/// classes they share.
fn names() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest name every kind admits", "dup4".to_string()),
        (
            "a name at the 64-character bound the strictest kind publishes",
            "n".repeat(64),
        ),
        (
            "hyphens and digits",
            "orders-by-region-2".to_string(),
        ),
        ("mixed case and digits", "DupName42".to_string()),
    ]
}

// ---------------------------------------------------------------------------
// What each kind needs before one of its resources can be created
// ---------------------------------------------------------------------------

/// The resources the creates below name: the public keys a key group lists, the key value
/// store a connection function associates and the multi-tenant distribution a tenant is
/// created under. Built once per emulator, so every row names resources this service really
/// holds.
struct Fixture {
    public_keys: Vec<String>,
    key_value_store_arn: String,
    distribution_id: String,
}

/// Stand up everything [`Fixture`] holds, over the one router under test.
async fn fixture(router: &MultiServiceRouter) -> Fixture {
    Fixture {
        public_keys: mint_public_keys(router, "dup-name-conflict", 3).await,
        key_value_store_arn: key_value_store_arn(router, "dup-name-conflict-store").await,
        distribution_id: multi_tenant_distribution(router, "ref-dup-name-conflict").await,
    }
}

// ---------------------------------------------------------------------------
// The requests
// ---------------------------------------------------------------------------

/// One request this suite submits: the resource kind, the name under test, the table row it
/// belongs to and which of that kind's own configurations it carries.
struct Request<'a> {
    kind: Kind,
    name: &'a str,
    /// The row of [`names`] under test, keeping apart the values that have to be unique
    /// across rows for a reason other than the name.
    row: usize,
    /// Which configuration of the kind's own table the request carries.
    variant: usize,
}

/// The document one request carries: the name under test, over the configuration its
/// `variant` selects out of that kind's own table.
fn request_body(request: &Request<'_>, fixture: &Fixture) -> String {
    let Request {
        kind,
        name,
        row,
        variant,
    } = *request;
    match kind {
        // `Name` is the only required member of a `CreateConnectionGroupRequest`, so the
        // optional ones are what a repeat varies.
        Kind::ConnectionGroup => format!(
            "<CreateConnectionGroupRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <Name>{}</Name><Ipv6Enabled>{}</Ipv6Enabled><Enabled>{}</Enabled>\
             </CreateConnectionGroupRequest>",
            escape(name),
            variant.is_multiple_of(2),
            !variant.is_multiple_of(3)
        ),
        Kind::KeyGroup => {
            let keys = &fixture.public_keys;
            Group {
                case: name.to_string(),
                name: name.to_string(),
                items: keys[..=variant % keys.len()].to_vec(),
                comment: Some(format!("the key group of configuration {variant}")),
            }
            .xml()
        }
        Kind::OriginAccessControl => {
            let rows = control_table();
            rows[variant % rows.len()].with_name(name, name).xml()
        }
        Kind::ResponseHeadersPolicy => {
            let rows = response_headers_policy_table();
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the response headers policy table has the row chosen");
            ResponseHeadersPolicy {
                name: name.to_string(),
                ..chosen
            }
            .xml()
        }
        Kind::CachePolicy => {
            let rows = cache_policy_table();
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the cache policy table has the row chosen");
            CachePolicy {
                name: name.to_string(),
                ..chosen
            }
            .xml()
        }
        Kind::ConnectionFunction => {
            let rows = connection_function_table(&fixture.key_value_store_arn);
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the connection function table has the row chosen");
            ConnectionFunction {
                name: name.to_string(),
                ..chosen
            }
            .xml()
        }
        Kind::OriginRequestPolicy => {
            let rows = origin_request_policy_table();
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the origin request policy table has the row chosen");
            OriginRequestPolicy {
                name: name.to_string(),
                ..chosen
            }
            .xml()
        }
        // A tenant claims a domain as well as a name, and no two tenants may claim the same
        // domain — so the domain follows the ROW and the configuration variant rather than
        // the name, and a repeat under a taken name claims a domain of its own.
        Kind::DistributionTenant => format!(
            "<CreateDistributionTenantRequest \
             xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
             <DistributionId>{}</DistributionId><Name>{}</Name>\
             <Domains><member><Domain>tenant-{row}-{variant}.example.com</Domain></member>\
             </Domains>\
             <Parameters><member><Name>tenantName</Name><Value>t{row}x{variant}</Value>\
             </member></Parameters>{}</CreateDistributionTenantRequest>",
            escape(&fixture.distribution_id),
            escape(name),
            managed_certificate_request()
        ),
    }
}

/// Create one resource under the request's name, through the same wire path that kind's own
/// suite creates one over.
async fn create_named(
    router: &MultiServiceRouter,
    request: &Request<'_>,
    fixture: &Fixture,
) -> Answer {
    let body = request_body(request, fixture);
    match request.kind {
        Kind::ConnectionGroup => create_connection_group(router, &body).await,
        Kind::KeyGroup => create_group(router, &body).await,
        Kind::OriginAccessControl => create_control(router, &body).await,
        Kind::ResponseHeadersPolicy => create_response_headers_policy(router, &body).await,
        Kind::CachePolicy => create_cache_policy(router, &body).await,
        Kind::ConnectionFunction => create_connection_function(router, &body).await,
        Kind::OriginRequestPolicy => create_origin_request_policy(router, &body).await,
        Kind::DistributionTenant => create_distribution_tenant(router, &body).await,
    }
}

/// Move the request's name onto the resource `stored` names, at the version token it was
/// read at, through the route the model binds that kind's update to.
async fn update_to_name(
    router: &MultiServiceRouter,
    request: &Request<'_>,
    stored: &Created,
    fixture: &Fixture,
) -> Answer {
    let body = request_body(request, fixture);
    let (id, etag) = (stored.id.as_str(), stored.etag.as_str());
    match request.kind {
        // `UpdateKeyGroup` and `UpdateOriginRequestPolicy` are
        // `PUT /2020-05-31/<resource>/{Id}` carrying the configuration element the create
        // carries (`smithy_query(query="operation",operation="UpdateKeyGroup,UpdateOriginRequestPolicy")`).
        Kind::KeyGroup => {
            call(
                router,
                "PUT",
                &format!("/{API_VERSION}/key-group/{id}"),
                Some(&body),
                Some(etag),
            )
            .await
        }
        Kind::OriginRequestPolicy => {
            call(
                router,
                "PUT",
                &format!("/{API_VERSION}/origin-request-policy/{id}"),
                Some(&body),
                Some(etag),
            )
            .await
        }
        Kind::CachePolicy => update_cache_policy(router, id, &body, Some(etag)).await,
        other => panic!("case {other:?}: this kind has no name-taking update under test"),
    }
}

/// The configuration a repeat under a taken name carries: a DIFFERENT row of the kind's own
/// table, so a create that replaced the stored resource rather than refusing the name would
/// be visible in the read-back.
///
/// The one exception is the origin access control, whose conflict this service decides over
/// the WHOLE parameter set rather than the name alone, so there the repeat re-submits the
/// parameters the first create stored.
fn repeat_variant(kind: Kind, variant: usize) -> usize {
    match kind {
        Kind::OriginAccessControl => variant,
        _ => variant + 1,
    }
}

/// One resource this suite created: the identifier and the version token its create
/// minted.
struct Created {
    id: String,
    etag: String,
}

/// The identifier a create minted, or `""` when the answer carries none.
fn minted_id(answer: &Answer) -> String {
    answer.root().find_text("Id").unwrap_or_default().to_string()
}

/// The message one refusal carries, or `""`.
fn message(answer: &Answer) -> &str {
    answer.root().find_text("Message").unwrap_or_default()
}

/// Read the resource of `kind` that `id` names back, through the route the model binds
/// that kind's read to (`smithy_query(query="operation",operation="GetConnectionGroup,GetKeyGroup,GetOriginAccessControl,GetResponseHeadersPolicy,GetCachePolicy,GetOriginRequestPolicy,GetDistributionTenant")`).
///
/// A connection function is read through `DescribeConnectionFunction`: `GetConnectionFunction`
/// answers with the function's CODE as the payload rather than a document, so the describe is
/// the read that reports the stored record.
async fn read_back(router: &MultiServiceRouter, kind: Kind, id: &str) -> Answer {
    if kind == Kind::ConnectionFunction {
        return describe_connection_function(router, id, None).await;
    }
    let resource = match kind {
        Kind::ConnectionGroup => "connection-group",
        Kind::KeyGroup => "key-group",
        Kind::OriginAccessControl => "origin-access-control",
        Kind::ResponseHeadersPolicy => "response-headers-policy",
        Kind::CachePolicy => "cache-policy",
        Kind::OriginRequestPolicy => "origin-request-policy",
        Kind::DistributionTenant => "distribution-tenant",
        Kind::ConnectionFunction => unreachable!("the describe above answers for this kind"),
    };
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/{resource}/{id}"),
        None,
        None,
    )
    .await
}

/// Assert that `answer` is the conflict `code` with exactly `expected` as its message, and
/// that the sentence carries neither the `name` submitted nor the `id` of the resource that
/// holds it.
fn assert_conflict(case: &str, answer: &Answer, code: &str, expected: &str, name: &str, id: &str) {
    assert_eq!(
        answer.status, 409,
        "case {case}: expected HTTP 409 under a taken name, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        code,
        "case {case}: expected {code}, got {}",
        answer.raw
    );
    let actual = message(answer);
    assert_eq!(
        actual, expected,
        "case {case}: expected {expected:?}, got {actual:?}"
    );
    assert!(
        !actual.contains(name),
        "case {case}: the refusal must not carry the submitted name {name:?}, got {actual:?}"
    );
    assert!(
        id.is_empty() || !actual.contains(id),
        "case {case}: the refusal must not carry the identifier {id:?} of the resource that \
         holds the name, got {actual:?}"
    );
}

// ---------------------------------------------------------------------------
// The rules
// ---------------------------------------------------------------------------

/// A create under a name the account already holds is refused with the service's fixed
/// sentence for that kind's conflict: the status and the code the model binds, and a message
/// carrying nothing from the request or from the store — so the sentence is the same
/// whichever name was chosen.
#[tokio::test]
async fn a_create_under_a_taken_name_carries_the_services_fixed_sentence() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for (kind, code, expected) in KINDS {
        let mut sentences: Vec<String> = Vec::new();
        for (row, (name_case, name)) in names().iter().enumerate() {
            let case = format!("{kind:?} under {name_case}");
            let first = create_named(
                &router,
                &Request {
                    kind: *kind,
                    name,
                    row,
                    variant: row,
                },
                &fixture,
            )
            .await;
            assert_eq!(
                first.status, 201,
                "case {case}: expected HTTP 201 from the first create, got {} with {}",
                first.status, first.raw
            );
            let holder = Created {
                id: minted_id(&first),
                etag: first.header("etag").to_string(),
            };

            let repeat = create_named(
                &router,
                &Request {
                    kind: *kind,
                    name,
                    row,
                    variant: repeat_variant(*kind, row),
                },
                &fixture,
            )
            .await;
            assert_conflict(&case, &repeat, code, expected, name, &holder.id);
            sentences.push(message(&repeat).to_string());

            // The refused create stored nothing: the resource that holds the name still
            // answers under the identity and the version token its OWN create minted, so a
            // kind that had replaced it with the differing configuration the repeat carried
            // fails here.
            let reread = read_back(&router, *kind, &holder.id).await;
            assert_eq!(
                reread.status, 200,
                "case {case}: expected the resource holding the name to survive the refusal, \
                 got {} with {}",
                reread.status, reread.raw
            );
            assert_eq!(
                reread.header("etag"),
                holder.etag,
                "case {case}: expected the version token {:?} the create minted, got {:?}",
                holder.etag,
                reread.header("etag")
            );
            let read_id = minted_id(&reread);
            assert!(
                read_id.is_empty() || read_id == holder.id,
                "case {case}: expected the read to answer for {:?}, got {read_id:?}",
                holder.id
            );
        }

        // Nothing of the name chosen reaches the sentence, so every row of this kind read
        // the same one.
        assert!(
            sentences.windows(2).all(|pair| pair[0] == pair[1]),
            "case {kind:?}: expected one sentence for every name, got {sentences:?}"
        );
    }
}

/// The other entry point into the same uniqueness rule: an update that moves onto this
/// resource a name a sibling already holds is refused with the one sentence that kind's
/// create is refused with, and both resources are left as they were.
#[tokio::test]
async fn an_update_onto_a_taken_name_carries_the_same_sentence() {
    let router = emulator();
    let fixture = fixture(&router).await;

    for kind in UPDATED_KINDS {
        let (code, expected) = KINDS
            .iter()
            .find_map(|(candidate, code, expected)| {
                (candidate == kind).then_some((*code, *expected))
            })
            .expect("every updated kind is a kind under test");

        for (row, (name_case, name)) in names().iter().enumerate() {
            let case = format!("{kind:?} updated onto {name_case}");
            // The resource that HOLDS the name, and the one whose update tries to take it.
            let holder = create_named(
                &router,
                &Request {
                    kind: *kind,
                    name,
                    row,
                    variant: row,
                },
                &fixture,
            )
            .await;
            assert_eq!(
                holder.status, 201,
                "case {case}: expected HTTP 201 for the resource holding the name, got {} with \
                 {}",
                holder.status, holder.raw
            );
            let holder_id = minted_id(&holder);

            let other_name = format!("other-{row}");
            let other = create_named(
                &router,
                &Request {
                    kind: *kind,
                    name: &other_name,
                    row,
                    variant: row + 1,
                },
                &fixture,
            )
            .await;
            assert_eq!(
                other.status, 201,
                "case {case}: expected HTTP 201 for the resource being updated, got {} with {}",
                other.status, other.raw
            );
            let updated = Created {
                id: minted_id(&other),
                etag: other.header("etag").to_string(),
            };

            let conflict = update_to_name(
                &router,
                &Request {
                    kind: *kind,
                    name,
                    row,
                    variant: row + 1,
                },
                &updated,
                &fixture,
            )
            .await;
            assert_conflict(&case, &conflict, code, expected, name, &holder_id);

            // A refused update changes nothing: the resource it addressed still answers at
            // the version token its own create minted.
            let reread = update_to_name(
                &router,
                &Request {
                    kind: *kind,
                    name: &other_name,
                    row,
                    variant: row + 1,
                },
                &updated,
                &fixture,
            )
            .await;
            assert_eq!(
                reread.status, 200,
                "case {case}: expected the refused update to leave the version token usable, \
                 got {} with {}",
                reread.status, reread.raw
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The kinds the harness above cannot drive
// ---------------------------------------------------------------------------

/// The remaining kinds whose create refuses a value the account already holds: the two
/// addressed by NAME rather than by a minted `Id` (a function and a key value store, which
/// the harness above cannot read back by id) and the one whose conflict is over the
/// `CallerReference` rather than the name (a public key).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NamedKind {
    Function,
    KeyValueStore,
    PublicKey,
}

/// Each of them, with the error code the model binds for its conflict and the message the
/// service states for that code — `""` where the service sends none at all.
const NAMED_KINDS: &[(NamedKind, &str, &str)] = &[
    (NamedKind::Function, "FunctionAlreadyExists", ""),
    (NamedKind::KeyValueStore, "EntityAlreadyExists", ""),
    (
        NamedKind::PublicKey,
        "PublicKeyAlreadyExists",
        "The caller reference that you are using to create a CloudFront public key is \
         associated with another public key in your account.",
    ),
];

/// The values these kinds are driven over: `FunctionName` and `KeyValueStoreName` are
/// `^[a-zA-Z0-9-_]{1,64}$` (`smithy_query(query="operation",operation="CreateFunction,CreateKeyValueStore")`),
/// and a `CallerReference` carries no modelled constraint, so these rows sit inside what all
/// three admit: the shortest form, the 64-character bound, and the character classes they
/// share.
fn named_values() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest value every kind admits", "dup5".to_string()),
        ("a value at the 64-character bound", "n".repeat(64)),
        ("underscores and digits", "orders_by_region_7".to_string()),
        ("mixed case and digits", "DupName43".to_string()),
    ]
}

/// A create of `kind` under `value`, carrying the configuration `variant` selects so a
/// repeat differs from the stored record in everything but the value under test.
async fn create_under_value(
    router: &MultiServiceRouter,
    kind: NamedKind,
    value: &str,
    variant: usize,
    store_arn: &str,
) -> Answer {
    match kind {
        NamedKind::Function => {
            let rows = function_create_table(store_arn);
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the function table has the row chosen");
            let body = crate::function_lifecycle::Create {
                name: value.to_string(),
                ..chosen
            }
            .xml();
            create_function(router, &body).await
        }
        NamedKind::KeyValueStore => {
            let rows = key_value_store_table();
            let count = rows.len();
            let chosen = rows
                .into_iter()
                .nth(variant % count)
                .expect("the key value store table has the row chosen");
            let body = crate::key_value_store_lifecycle::Create {
                name: value.to_string(),
                ..chosen
            }
            .xml();
            create_key_value_store(router, &body).await
        }
        // The conflict is over the CALLER REFERENCE, so the reference is what repeats and
        // everything else — the name and the encoded material — differs with the variant.
        NamedKind::PublicKey => {
            let body = PublicKey {
                case: value.to_string(),
                caller_reference: value.to_string(),
                name: format!("{value}-{variant}"),
                encoded_key: pem(1 + variant % 3, 'K'),
                comment: Some(format!("the public key of configuration {variant}")),
            }
            .xml();
            create_key(router, &body).await
        }
    }
}

/// Read the resource `value` names back, through the reader the model binds for that kind.
async fn read_named_back(
    router: &MultiServiceRouter,
    kind: NamedKind,
    value: &str,
    holder_id: &str,
) -> Answer {
    match kind {
        NamedKind::Function => describe_function(router, value, None).await,
        NamedKind::KeyValueStore => describe_key_value_store(router, value).await,
        NamedKind::PublicKey => get_key(router, holder_id).await,
    }
}

/// The same rule over those kinds: the conflict is HTTP 409 under the code the model binds,
/// its message is the FIXED text the service states for that code — and where the service
/// states none, the refusal carries no message at all — and the resource holding the value
/// survives the refusal.
#[tokio::test]
async fn a_create_under_a_taken_name_or_reference_carries_the_services_fixed_sentence() {
    let router = emulator();
    // The key value store a function's configuration associates, so every function row names
    // a store this service really holds.
    let store_arn = function_key_value_store(&router, "dup_named_conflict_store").await;

    for (kind, code, expected) in NAMED_KINDS {
        let mut sentences: Vec<String> = Vec::new();
        for (row, (value_case, value)) in named_values().iter().enumerate() {
            let case = format!("{kind:?} under {value_case}");
            let first = create_under_value(&router, *kind, value, row, &store_arn).await;
            assert_eq!(
                first.status, 201,
                "case {case}: expected HTTP 201 from the first create, got {} with {}",
                first.status, first.raw
            );
            let holder_id = minted_id(&first);
            let holder_etag = first.header("etag").to_string();

            let repeat = create_under_value(&router, *kind, value, row + 1, &store_arn).await;
            assert_conflict(&case, &repeat, code, expected, value, &holder_id);
            sentences.push(message(&repeat).to_string());

            // The refused create stored nothing: the resource that holds the value still
            // answers at the version token its OWN create minted.
            let reread = read_named_back(&router, *kind, value, &holder_id).await;
            assert_eq!(
                reread.status, 200,
                "case {case}: expected the resource holding the value to survive the refusal, \
                 got {} with {}",
                reread.status, reread.raw
            );
            assert_eq!(
                reread.header("etag"),
                holder_etag,
                "case {case}: expected the version token {holder_etag:?} the create minted, got \
                 {:?}",
                reread.header("etag")
            );
        }

        // Nothing of the value chosen reaches the sentence, so every row read the same one.
        assert!(
            sentences.windows(2).all(|pair| pair[0] == pair[1]),
            "case {kind:?}: expected one sentence for every value, got {sentences:?}"
        );
    }
}
