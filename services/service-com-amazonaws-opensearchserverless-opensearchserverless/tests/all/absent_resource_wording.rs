// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS and CLASSIFIES a refusal about a resource that is not there (or
//! is already there, or is not in a state the write may start from), end to end over the
//! wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: each such refusal carries the error class the ADDRESSED operation
//! models for that condition, and a message built from the resource kind as this service
//! names it plus the identifier (or the name and type, or the statuses) the REQUEST and the
//! stored state supply — so the one sentence a client keys on is the same whichever
//! operation of that resource produced it, and never a sentence about a different resource
//! or a narrower precondition.
//!
//! The tables span what the rule generalises over: the resource (collection group,
//! security configuration, collection, managed interface VPC endpoint), the operation
//! addressing it (read, update, delete, create, batch read) and so the error class it
//! models, the form of the identifier (both published bounds of its shape, a suffix that is
//! empty, an id that encodes a name and type and one that does not), and whether the
//! identifier never named anything or named something that has since been deleted.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::client_token_scope::await_collection_active;
use crate::collection_group_lifecycle::create_group;
use crate::collection_lifecycle::{at, call, emulator, text, ACCOUNT, CLIENT_ERROR};
use crate::security_configs::{create_config, saml_options, INSTANCE_ARN};
use crate::tag_key_uniqueness::create_collection;
use crate::vpc_endpoint::{create_endpoint, read_endpoints};

/// Every `SecurityConfigType` the model declares
/// (`smithy_query(query="shape", shape="SecurityConfigType")` -> `values`), with the
/// identity-provider options a create of that type carries.
fn security_config_types() -> Vec<(&'static str, Value)> {
    vec![
        ("saml", json!({ "samlOptions": saml_options() })),
        (
            "iamidentitycenter",
            json!({ "iamIdentityCenterOptions": { "instanceArn": INSTANCE_ARN } }),
        ),
        (
            "iamfederation",
            json!({ "iamFederationOptions": { "userAttribute": "Email" } }),
        ),
    ]
}

/// A `CreateSecurityConfig` request for `kind` named `name`.
fn create_config_request(kind: &str, name: &str, options: &Value) -> Value {
    let mut request = json!({ "type": kind, "name": name });
    if let Some(members) = options.as_object() {
        for (member, value) in members {
            request[member] = value.clone();
        }
    }
    request
}

/// How this service words a security configuration a WRITE addressed and did not find.
fn policy_absent(name: &str, kind: &str) -> String {
    format!("Policy with name {name} and type {kind} is not found")
}

/// How this service words a security configuration a READ addressed and did not find.
fn config_absent(id: &str) -> String {
    format!("Security config with id: {id} does not exist")
}

/// Assert `operation` refused `request` with `code` and exactly `message`.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: Value,
    code: &str,
    message: &str,
) {
    let (status, answered, answer) = call(router, operation, request).await;
    assert_eq!(
        (status, answered.as_str()),
        (CLIENT_ERROR, code),
        "case {case}: expected {CLIENT_ERROR} {code} from {operation}, got {status} \
         {answered} with {answer}"
    );
    assert_eq!(
        text(&answer, "message"),
        message,
        "case {case}: expected {operation} to answer {message:?}, got {answer}"
    );
}

// ---------------------------------------------------------------------------
// The collection group: the delete names the id it was given, whether that id never named
// a group or named one that has since been deleted.
// ---------------------------------------------------------------------------

/// How this service words a collection group a request addressed by id and did not find.
fn group_absent(id: &str) -> String {
    format!("CollectionGroup with ID '{id}' cannot be found.")
}

#[tokio::test]
async fn deleting_a_collection_group_id_that_names_none_names_that_id() {
    let router = emulator();

    // One group is stored throughout, so each refusal below is about the id it was given
    // and not about an empty account.
    let kept = create_group(
        &router,
        "us-east-1",
        json!({ "name": "kept-group", "standbyReplicas": "ENABLED" }),
    )
    .await;

    // Every row satisfies `CollectionGroupId`'s own `^[a-z0-9]{3,40}$`, so each reaches the
    // handler: both published length bounds, one in between, and one of digits alone.
    let longest = "z9".repeat(20);
    let cases: Vec<(&str, String)> = vec![
        ("an id at the minimum length of three", "abc".to_string()),
        ("an id at the maximum length of 40", longest),
        ("an id in between", "0123456789abcdef".to_string()),
        ("an id of digits alone", "00000000000000000000".to_string()),
    ];

    for (case, id) in &cases {
        refused(
            &router,
            case,
            "DeleteCollectionGroup",
            json!({ "id": id }),
            "ResourceNotFoundException",
            &group_absent(id),
        )
        .await;
    }

    // The same wording for an id that DID name a group until it was deleted, which is the
    // other way a client reaches this refusal — and the id is the one the create minted.
    let doomed = create_group(
        &router,
        "us-east-1",
        json!({ "name": "doomed-group", "standbyReplicas": "DISABLED" }),
    )
    .await;
    let doomed_id = text(&doomed, "id");
    let (status, code, answer) =
        call(&router, "DeleteCollectionGroup", json!({ "id": &doomed_id })).await;
    assert_eq!(
        status, 200,
        "case the first delete of {doomed_id}: expected 200, got {status} {code} with {answer}"
    );
    refused(
        &router,
        "after the delete",
        "DeleteCollectionGroup",
        json!({ "id": &doomed_id }),
        "ResourceNotFoundException",
        &group_absent(&doomed_id),
    )
    .await;

    // Nothing above touched the stored group: it is still updatable, which is what proves
    // the refusals were about the ids and not about the store.
    let kept_id = text(&kept, "id");
    let (status, code, answer) = call(
        &router,
        "UpdateCollectionGroup",
        json!({ "id": &kept_id, "description": "still here" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the stored group after every refusal: expected 200, got {status} {code} with \
         {answer}"
    );
    assert_eq!(
        text(&answer, "updateCollectionGroupDetail.description"),
        "still here",
        "case the stored group after every refusal: expected the update to be applied, got \
         {answer}"
    );
}

// ---------------------------------------------------------------------------
// The security configuration: the read names the id, the writes name the policy the id
// encodes.
// ---------------------------------------------------------------------------

/// One row of the security-configuration table: how the case reads, the `SecurityConfigId`
/// it addresses, and the `(name, type)` that id encodes when it encodes one at all.
struct AbsentConfigId {
    case: String,
    id: String,
    encoded: Option<(String, String)>,
}

#[tokio::test]
async fn the_security_config_writes_name_the_policy_their_id_encodes() {
    let router = emulator();

    // Rows differ along the `SecurityConfigType` the id names, the name it ends with (both
    // published bounds of `ConfigName`, `@length(min: 3, max: 32)`), and the SHAPE of the
    // id: the documented `<type>/<account>/<name>` form, a two-segment form, and three
    // forms that encode no name or type at all.
    let longest_name = format!("n{}", "a".repeat(31));
    let mut cases: Vec<AbsentConfigId> = vec![];
    for (kind, _) in security_config_types() {
        for name in ["abc", longest_name.as_str()] {
            cases.push(AbsentConfigId {
                case: format!("a {kind} id nothing created, name {} chars", name.len()),
                id: format!("{kind}/{ACCOUNT}/{name}"),
                encoded: Some((name.to_string(), kind.to_string())),
            });
        }
        cases.push(AbsentConfigId {
            case: format!("a two-segment {kind} id"),
            id: format!("{kind}/lonely-name"),
            encoded: Some(("lonely-name".to_string(), kind.to_string())),
        });
    }
    // An id that encodes neither a name nor a type: the writes have nothing to name it by,
    // so they name the id itself, exactly as the read always does.
    for (case, id) in [
        ("an id with no separator", "not-an-id".to_string()),
        ("an id with an empty name", format!("saml/{ACCOUNT}/")),
        ("an id with an empty type", "/only-a-name".to_string()),
    ] {
        cases.push(AbsentConfigId {
            case: case.to_string(),
            id,
            encoded: None,
        });
    }

    for AbsentConfigId { case, id, encoded } in &cases {
        let write_message = match encoded {
            Some((name, kind)) => policy_absent(name, kind),
            None => config_absent(id),
        };
        // The READ addresses the configuration by its id, so it names the id back.
        refused(
            &router,
            case,
            "GetSecurityConfig",
            json!({ "id": id }),
            "ResourceNotFoundException",
            &config_absent(id),
        )
        .await;
        refused(
            &router,
            case,
            "UpdateSecurityConfig",
            // `configVersion` is `@required`; this one is a well-formed `PolicyVersion`
            // nothing ever minted.
            json!({ "id": id, "configVersion": "MTAwMDAwMDAwMDAwMDAwMA==" }),
            "ResourceNotFoundException",
            &write_message,
        )
        .await;
        refused(
            &router,
            case,
            "DeleteSecurityConfig",
            json!({ "id": id }),
            "ResourceNotFoundException",
            &write_message,
        )
        .await;
    }

    // The same wording for a configuration that existed until it was deleted, for every
    // modelled type: the name and the type come out of the id the create minted.
    for (kind, options) in security_config_types() {
        let name = format!("doomed-{kind}");
        let created = create_config(&router, create_config_request(kind, &name, &options)).await;
        let id = text(&created, "id");
        let (status, code, answer) =
            call(&router, "DeleteSecurityConfig", json!({ "id": &id })).await;
        assert_eq!(
            status, 200,
            "case the first delete of {id}: expected 200, got {status} {code} with {answer}"
        );
        refused(
            &router,
            &format!("a deleted {kind} configuration"),
            "GetSecurityConfig",
            json!({ "id": &id }),
            "ResourceNotFoundException",
            &config_absent(&id),
        )
        .await;
        refused(
            &router,
            &format!("a deleted {kind} configuration"),
            "DeleteSecurityConfig",
            json!({ "id": &id }),
            "ResourceNotFoundException",
            &policy_absent(&name, kind),
        )
        .await;
    }
}

#[tokio::test]
async fn a_second_security_config_of_one_type_and_name_is_refused_as_a_policy_conflict() {
    let router = emulator();

    // Rows span every modelled type and both published bounds of `ConfigName`, so the
    // refusal is about the taken `(type, name)` pair and not about one name.
    let longest_name = format!("c{}", "x".repeat(31));
    for (kind, options) in security_config_types() {
        for name in ["abc", longest_name.as_str()] {
            let case = format!("{kind}/{name}");
            let request = create_config_request(kind, name, &options);
            let created = create_config(&router, request.clone()).await;
            let version = text(&created, "configVersion");

            refused(
                &router,
                &case,
                "CreateSecurityConfig",
                request,
                "ConflictException",
                &format!("Policy with name {name} and type {kind} already exists"),
            )
            .await;

            // The refused create left the stored configuration exactly as it was: the read
            // still answers the first create's version and type.
            let (status, code, answer) = call(
                &router,
                "GetSecurityConfig",
                json!({ "id": text(&created, "id") }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from GetSecurityConfig, got {status} {code} with \
                 {answer}"
            );
            assert_eq!(
                text(&answer, "securityConfigDetail.configVersion"),
                version,
                "case {case}: expected the refused create to leave the stored version alone, \
                 got {answer}"
            );
            assert_eq!(
                text(&answer, "securityConfigDetail.type"),
                kind,
                "case {case}: expected the stored configuration to keep its type, got {answer}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The collection: an update refused for the status it was attempted from names EVERY
// status it would have been accepted in.
// ---------------------------------------------------------------------------

/// The statuses an update IS accepted in, as the refusal names them: a collection that is
/// ready to use, and one whose last update failed and is therefore retryable.
const UPDATABLE_STATUSES: &str = "ACTIVE or UPDATE_FAILED";

/// How this service words a collection that is not in a status it may be updated from.
fn not_updatable(id: &str) -> String {
    format!("Collection Id {id} cannot be updated because it is not in {UPDATABLE_STATUSES} Status")
}

/// Create a collection named `name` with its own encryption setting, answering its minted
/// id. The create itself is the harness's own [`create_collection`], reused rather than
/// re-implemented.
async fn create_named_collection(router: &MultiServiceRouter, name: &str) -> String {
    let detail = create_collection(
        router,
        json!({ "name": name, "encryptionConfig": { "aWSOwnedKey": true } }),
    )
    .await;
    text(&detail, "id")
}

#[tokio::test]
async fn an_update_of_a_collection_in_an_unupdatable_status_names_every_updatable_status() {
    let router = emulator();

    // Each case reaches `UpdateCollection` with the collection in one of the non-terminal
    // statuses it passes through, and names a different member each time, so the refusal is
    // about the STATUS and not about what the update asked for. The transient statuses are
    // owed back to the clock, so each refusal is attempted straight after the call that
    // left the collection in one.

    // Still CREATING: the create answers before the `CREATING -> ACTIVE` move falls due.
    let creating = create_named_collection(&router, "creating-collection").await;
    refused(
        &router,
        "still CREATING",
        "UpdateCollection",
        json!({ "id": &creating, "description": "while creating" }),
        "ConflictException",
        &not_updatable(&creating),
    )
    .await;

    // Still UPDATING: `vectorOptions` is the member that reconfigures what the collection
    // RUNS, so an accepted update of it leaves the collection `UPDATING` until that work
    // falls due.
    let updating = create_named_collection(&router, "updating-collection").await;
    await_collection_active(&router, "the collection to be reconfigured", &updating).await;
    let (status, code, answer) = call(
        &router,
        "UpdateCollection",
        json!({ "id": &updating, "vectorOptions": { "ServerlessVectorAcceleration": "ENABLED" } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the reconfiguring update: expected 200, got {status} {code} with {answer}"
    );
    refused(
        &router,
        "still UPDATING",
        "UpdateCollection",
        json!({ "id": &updating, "deletionProtection": "ENABLED" }),
        "ConflictException",
        &not_updatable(&updating),
    )
    .await;

    // Already DELETING: an accepted delete leaves the collection readable in `DELETING`
    // until its purge falls due.
    let deleting = create_named_collection(&router, "deleting-collection").await;
    await_collection_active(&router, "the collection to be deleted", &deleting).await;
    let (status, code, answer) =
        call(&router, "DeleteCollection", json!({ "id": &deleting })).await;
    assert_eq!(
        status, 200,
        "case the accepted delete: expected 200, got {status} {code} with {answer}"
    );
    refused(
        &router,
        "already DELETING",
        "UpdateCollection",
        json!({
            "id": &deleting,
            "vectorOptions": { "ServerlessVectorAcceleration": "DISABLED" },
        }),
        "ConflictException",
        &not_updatable(&deleting),
    )
    .await;

    // An id that names no collection at all is a different cause with its own wording, so
    // the status sentence is not the answer to every refused update.
    refused(
        &router,
        "an id naming no collection",
        "UpdateCollection",
        json!({ "id": "0".repeat(20), "description": "nothing to update" }),
        "ValidationException",
        &format!("Collection with ID {} is not found", "0".repeat(20)),
    )
    .await;

    // From a settled `ACTIVE` the same update IS accepted, and the store proves it: the
    // precondition refuses a status, it does not refuse the request.
    await_collection_active(&router, "the reconfigured collection", &updating).await;
    let (status, code, answer) = call(
        &router,
        "UpdateCollection",
        json!({ "id": &updating, "description": "accepted from ACTIVE" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case the update from ACTIVE: expected 200, got {status} {code} with {answer}"
    );
    let detail = await_collection_active(&router, "the update from ACTIVE", &updating).await;
    assert_eq!(
        text(&detail, "description"),
        "accepted from ACTIVE",
        "case the update from ACTIVE: expected the read to report the applied update, got \
         {detail}"
    );
}

// ---------------------------------------------------------------------------
// The managed interface VPC endpoint: three operations, three classes, one id rendered in
// each.
// ---------------------------------------------------------------------------

/// How `DeleteVpcEndpoint`, which models `ResourceNotFoundException`, words an id that
/// names no endpoint.
fn endpoint_not_found(id: &str) -> String {
    format!("VpcEndpoint with ID '{id}' cannot be found.")
}

/// How `UpdateVpcEndpoint`, which models NO not-found error, words the same cause.
fn endpoint_conflict(id: &str) -> String {
    format!("The VpcEndpoint with Id '{id}' is not exist.")
}

/// The `errorMessage` a `VpcEndpointErrorDetail` carries for the same cause, inside an
/// otherwise successful batch read.
const ENDPOINT_DETAIL_MESSAGE: &str = "The specified VpcEndpoint is not found.";

/// The `errorCode` this service's batch readers report for an identifier that names nothing.
const NOT_FOUND_CODE: &str = "NOT_FOUND";

/// Assert every operation that can be told about `id` reports its absence in that
/// operation's own class and wording.
async fn assert_endpoint_absent(router: &MultiServiceRouter, case: &str, id: &str) {
    refused(
        router,
        case,
        "DeleteVpcEndpoint",
        json!({ "id": id }),
        "ResourceNotFoundException",
        &endpoint_not_found(id),
    )
    .await;
    refused(
        router,
        case,
        "UpdateVpcEndpoint",
        json!({ "id": id, "addSubnetIds": ["subnet-1a2b3c4d"] }),
        "ConflictException",
        &endpoint_conflict(id),
    )
    .await;

    // The batch read is not a refusal at all: it answers 200 with one error detail per id
    // that named nothing.
    let answer = read_endpoints(router, case, &[id]).await;
    let error = at(&answer, "vpcEndpointErrorDetails")
        .as_array()
        .and_then(|errors| errors.first().cloned())
        .unwrap_or(Value::Null);
    assert_eq!(
        text(&error, "id"),
        id,
        "case {case}: expected the error detail to carry the id the request named, got {answer}"
    );
    assert_eq!(
        text(&error, "errorCode"),
        NOT_FOUND_CODE,
        "case {case}: expected the {NOT_FOUND_CODE} error code, got {answer}"
    );
    assert_eq!(
        text(&error, "errorMessage"),
        ENDPOINT_DETAIL_MESSAGE,
        "case {case}: expected the batch reader's not-found wording, got {answer}"
    );
}

#[tokio::test]
async fn each_vpc_endpoint_operation_reports_a_missing_endpoint_in_its_own_class() {
    let router = emulator();

    // One endpoint is stored throughout, so each refusal is about the id it was given.
    let kept = create_endpoint(
        &router,
        "the endpoint kept throughout",
        json!({
            "name": "kept-endpoint",
            "vpcId": "vpc-1a2b3c4d",
            "subnetIds": ["subnet-1a2b3c4d"],
        }),
    )
    .await;

    // Every row satisfies `VpcEndpointId`'s own `^vpce-[0-9a-z]*$` and
    // `@length(min: 1, max: 255)`, so each reaches the handler: a never-minted id of the
    // width AWS uses, an empty suffix, a one-character suffix, and a suffix at the modelled
    // maximum length.
    let longest = format!("vpce-{}", "a".repeat(250));
    let cases: Vec<(&str, String)> = vec![
        ("a never-minted id", "vpce-00000000000000000".to_string()),
        ("an empty suffix", "vpce-".to_string()),
        ("a one-character suffix", "vpce-1".to_string()),
        ("a suffix at the maximum length", longest),
    ];

    for (case, id) in &cases {
        assert_endpoint_absent(&router, case, id).await;
    }

    // The stored endpoint is untouched by all of it: it still answers the read, and its own
    // id is served rather than refused.
    let kept_id = text(&kept, "id");
    let answer = read_endpoints(&router, "the stored endpoint", &[kept_id.as_str()]).await;
    assert_eq!(
        at(&answer, "vpcEndpointErrorDetails")
            .as_array()
            .map(Vec::len)
            .unwrap_or_default(),
        0,
        "case the stored endpoint: expected no error detail for a stored id, got {answer}"
    );
}
