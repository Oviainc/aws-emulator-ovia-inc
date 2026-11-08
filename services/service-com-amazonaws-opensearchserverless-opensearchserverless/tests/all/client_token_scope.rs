// SPDX-License-Identifier: CC0-1.0

//! The SCOPE of an `@idempotencyToken` replay, end to end over the wire.
//!
//! Every case drives the real request path through [`crate::collection_lifecycle::call`] —
//! `MultiServiceRouter::dispatch` with `POST /`, `X-Amz-Target:
//! OpenSearchServerless.<Operation>` and an `awsJson1_0` body — so the boundary validator,
//! the protocol codec and the handler all take part, exactly as they do for an SDK client.
//!
//! The rule under test: a replayed `clientToken` answers the record the first call left only
//! when the request addresses THAT resource; a token spent again against another resource is
//! a request for that other resource, so its write is applied and the answer describes the
//! resource the request addressed. The table spans the updates this service serves and both
//! forms of addressed identity — a server-minted `id` (`UpdateCollection`,
//! `UpdateVpcEndpoint`) and a `(type, name)` pair (`UpdateSecurityPolicy`) — and the member
//! each row changes: a free-text `description` and a set of `subnetIds`, including both
//! modelled `SubnetId` widths.

use serde_json::{json, Value};

use crate::collection_lifecycle::{at, call, emulator, text};
use crate::encryption_indexes_and_tagging::active_collection;
use crate::vpc_endpoint::{await_active, create_endpoint, read_one, SETTLE_WAIT};

/// The `VpcId` every endpoint here is created in; `VpcId`'s modelled pattern is
/// `^vpc-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="VpcId")`).
const VPC_ID: &str = "vpc-0123456789abcdef0";

/// The subnet every endpoint here is created with, which no row changes.
const BASE_SUBNET: &str = "subnet-00000001";

/// A subnet of the SHORT modelled width and one of the LONG one: `SubnetId`'s pattern is
/// `^subnet-([0-9a-f]{8}|[0-9a-f]{17})$` (`smithy_query(query="shape", shape="SubnetId")`),
/// so these are its two alternatives, pinned by the width checks below.
const NARROW_SUBNET: &str = "subnet-0000000a";
const WIDE_SUBNET: &str = "subnet-0000000000000000b";
const _: () = assert!(NARROW_SUBNET.len() == "subnet-".len() + 8);
const _: () = assert!(WIDE_SUBNET.len() == "subnet-".len() + 17);

/// The `SecurityPolicyType` the policy row uses, from
/// `smithy_query(query="shape", shape="SecurityPolicyType")` -> `encryption | network`.
const POLICY_TYPE: &str = "network";

/// A `network` policy document, shaped as AWS's "Network policies for Amazon OpenSearch
/// Serverless" documents it.
const POLICY_DOCUMENT: &str =
    r#"[{"Rules":[{"ResourceType":"dashboard","Resource":["collection/scoped*"]}],"AllowFromPublic":true}]"#;

/// The update one row drives, which is also what addresses its resource.
#[derive(Clone, Copy, Debug)]
enum Update {
    /// Addressed by the server-minted `CollectionId`.
    Collection,
    /// Addressed by the server-minted `VpcEndpointId`.
    VpcEndpoint,
    /// Addressed by the `(type, name)` pair.
    SecurityPolicy,
}

impl Update {
    /// The operation this row calls.
    fn operation(self) -> &'static str {
        match self {
            Update::Collection => "UpdateCollection",
            Update::VpcEndpoint => "UpdateVpcEndpoint",
            Update::SecurityPolicy => "UpdateSecurityPolicy",
        }
    }

    /// The output member the operation reports its record in
    /// (`smithy_query(query="operation", operation="UpdateCollection,UpdateVpcEndpoint,UpdateSecurityPolicy")`).
    fn detail_member(self) -> &'static str {
        match self {
            Update::Collection => "updateCollectionDetail",
            Update::VpcEndpoint => "UpdateVpcEndpointDetail",
            Update::SecurityPolicy => "securityPolicyDetail",
        }
    }
}

/// One stored resource an update addresses.
#[derive(Clone, Debug)]
struct Resource {
    /// The members that ADDRESS it on an update request — its whole identity.
    address: Value,
    /// The write precondition the operation models, when it has one.
    version: Option<String>,
}

/// Create the resource `label` names for `update`, settled into the status it may be
/// changed from, and answer what addresses it.
async fn create_resource(
    router: &smithy_server_runtime::MultiServiceRouter,
    update: Update,
    case: &str,
    label: &str,
) -> Resource {
    let name = format!("scoped-{label}");
    match update {
        Update::Collection => {
            let detail = active_collection(router, &name).await;
            Resource {
                address: json!({ "id": text(&detail, "id") }),
                version: None,
            }
        }
        Update::VpcEndpoint => {
            let detail = create_endpoint(
                router,
                case,
                json!({ "name": name, "vpcId": VPC_ID, "subnetIds": [BASE_SUBNET] }),
            )
            .await;
            let id = text(&detail, "id");
            // An endpoint may only be CHANGED once it has settled, so the row starts from
            // the status the clock leaves it in rather than from the create's own.
            await_active(router, case, &id).await;
            Resource {
                address: json!({ "id": id }),
                version: None,
            }
        }
        Update::SecurityPolicy => {
            let (status, code, answer) = call(
                router,
                "CreateSecurityPolicy",
                json!({ "type": POLICY_TYPE, "name": name, "policy": POLICY_DOCUMENT }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from CreateSecurityPolicy, got {status} {code} with {answer}"
            );
            let detail = at(&answer, "securityPolicyDetail");
            Resource {
                address: json!({ "type": POLICY_TYPE, "name": name }),
                version: Some(text(detail, "policyVersion")),
            }
        }
    }
}

/// The update request that writes `marker` into `resource`, carrying `token`.
fn update_request(update: Update, resource: &Resource, marker: &str, token: &str) -> Value {
    let mut request = resource.address.clone();
    match update {
        Update::Collection | Update::SecurityPolicy => request["description"] = json!(marker),
        Update::VpcEndpoint => request["addSubnetIds"] = json!([marker]),
    }
    if let Some(version) = &resource.version {
        request["policyVersion"] = json!(version);
    }
    request["clientToken"] = json!(token);
    request
}

/// Call `update`'s operation with `request`, asserting it was accepted, and answer the
/// record it reports.
async fn accepted(
    router: &smithy_server_runtime::MultiServiceRouter,
    update: Update,
    case: &str,
    request: Value,
) -> Value {
    let operation = update.operation();
    let (status, code, answer) = call(router, operation, request).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200 from {operation}, got {status} {code} with {answer}"
    );
    at(&answer, update.detail_member()).clone()
}

/// The markers `detail` carries, rendered as one string so a case can assert which writes
/// reached the record it describes.
fn carried_markers(update: Update, detail: &Value) -> String {
    match update {
        Update::Collection | Update::SecurityPolicy => text(detail, "description"),
        Update::VpcEndpoint => at(detail, "subnetIds")
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .map(|entry| entry.as_str().unwrap_or_default())
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default(),
    }
}

/// The markers the STORE holds for `resource`, read back through this service's reader once
/// the resource has settled.
async fn stored_markers(
    router: &smithy_server_runtime::MultiServiceRouter,
    update: Update,
    case: &str,
    resource: &Resource,
) -> String {
    let detail = match update {
        Update::Collection => {
            let id = text(&resource.address, "id");
            await_collection_active(router, case, &id).await
        }
        Update::VpcEndpoint => {
            let id = text(&resource.address, "id");
            await_active(router, case, &id).await
        }
        Update::SecurityPolicy => {
            let (status, code, answer) =
                call(router, "GetSecurityPolicy", resource.address.clone()).await;
            assert_eq!(
                status, 200,
                "case {case}: expected 200 from GetSecurityPolicy, got {status} {code} with {answer}"
            );
            at(&answer, "securityPolicyDetail").clone()
        }
    };
    carried_markers(update, &detail)
}

/// Poll `BatchGetCollection` until `id` reports `ACTIVE`, answering its `CollectionDetail`.
///
/// The `UPDATING -> ACTIVE` move is owed by the clock, so this waits for the wall clock
/// rather than for a call.
pub(crate) async fn await_collection_active(
    router: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    id: &str,
) -> Value {
    for _ in 0..10 {
        let (status, code, answer) =
            call(router, "BatchGetCollection", json!({ "ids": [id] })).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200 from BatchGetCollection, got {status} {code} with {answer}"
        );
        let detail = at(&answer, "collectionDetails")
            .as_array()
            .and_then(|details| details.first())
            .cloned()
            .unwrap_or(Value::Null);
        if text(&detail, "status") == "ACTIVE" {
            return detail;
        }
        tokio::time::sleep(SETTLE_WAIT / 5).await;
    }
    panic!("case {case}: the collection {id} never reached ACTIVE");
}

/// Assert `detail` describes the resource `resource` addresses, member by member.
fn assert_addresses(case: &str, resource: &Resource, detail: &Value) {
    let address = resource
        .address
        .as_object()
        .expect("an address is a JSON object");
    for (member, value) in address {
        assert_eq!(
            at(detail, member),
            value,
            "case {case}: expected the answer to report {member} {value}, got {detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// A token spent again on ANOTHER resource is a request for that other resource.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_is_scoped_to_the_resource_the_request_addresses() {
    // Each row: the update under test, and the marker each of its two resources is changed
    // with. The two subnet markers are the two modelled `SubnetId` widths (8 and 17 hex
    // digits, `smithy_query(query="shape", shape="SubnetId")`).
    let rows: Vec<(Update, &str, &str)> = vec![
        (
            Update::Collection,
            "the change the first collection asked for",
            "the change the second collection asked for",
        ),
        (Update::VpcEndpoint, NARROW_SUBNET, WIDE_SUBNET),
        (
            Update::SecurityPolicy,
            "the change the first policy asked for",
            "the change the second policy asked for",
        ),
    ];

    for (update, marker_first, marker_second) in rows {
        // One emulator per row, so a row is answered only out of what it stored itself.
        let router = emulator();
        let operation = update.operation();
        let token = "one-spent-client-token";

        let first = create_resource(&router, update, &format!("{operation} first"), "first").await;
        let second =
            create_resource(&router, update, &format!("{operation} second"), "second").await;

        // The token's FIRST call: an ordinary update of the first resource.
        let case = format!("{operation}: the first call");
        let detail = accepted(
            &router,
            update,
            &case,
            update_request(update, &first, marker_first, token),
        )
        .await;
        assert_addresses(&case, &first, &detail);
        assert!(
            carried_markers(update, &detail).contains(marker_first),
            "case {case}: expected the answer to carry {marker_first:?}, got {detail}"
        );

        // The SAME token against the other resource: a request for that other resource,
        // which is served on its own terms rather than answered with the first record.
        let case = format!("{operation}: the token spent again on another resource");
        let detail = accepted(
            &router,
            update,
            &case,
            update_request(update, &second, marker_second, token),
        )
        .await;
        assert_addresses(&case, &second, &detail);
        let carried = carried_markers(update, &detail);
        assert!(
            carried.contains(marker_second),
            "case {case}: expected the answer to carry the change it asked for ({marker_second:?}), got {detail}"
        );
        assert!(
            !carried.contains(marker_first),
            "case {case}: expected the answer to carry nothing of the first resource ({marker_first:?}), got {detail}"
        );

        // The store agrees: each resource holds its OWN change and nothing of the other's.
        let case = format!("{operation}: the stored second resource");
        let stored = stored_markers(&router, update, &case, &second).await;
        assert!(
            stored.contains(marker_second),
            "case {case}: expected the write it accepted to be stored ({marker_second:?}), got {stored:?}"
        );
        assert!(
            !stored.contains(marker_first),
            "case {case}: expected nothing of the first resource ({marker_first:?}), got {stored:?}"
        );
        let case = format!("{operation}: the stored first resource");
        let stored = stored_markers(&router, update, &case, &first).await;
        assert!(
            stored.contains(marker_first),
            "case {case}: expected its own change to be stored ({marker_first:?}), got {stored:?}"
        );
        assert!(
            !stored.contains(marker_second),
            "case {case}: expected nothing of the second resource ({marker_second:?}), got {stored:?}"
        );

        // A RETRY of the token's first request still replays the record that call left: the
        // token names the request it was first spent on, so scoping the replay has not
        // turned a retry into a second write.
        let case = format!("{operation}: the retry of the first request");
        let detail = accepted(
            &router,
            update,
            &case,
            update_request(update, &first, marker_first, token),
        )
        .await;
        assert_addresses(&case, &first, &detail);
        let carried = carried_markers(update, &detail);
        assert!(
            carried.contains(marker_first),
            "case {case}: expected the record the first call left ({marker_first:?}), got {detail}"
        );
        assert!(
            !carried.contains(marker_second),
            "case {case}: expected nothing of the second resource ({marker_second:?}), got {detail}"
        );
    }
}

// ---------------------------------------------------------------------------
// A token spent again on another resource does not stop that resource from being
// addressed on its own terms: the refusals each update owes are still answered.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_spent_token_does_not_answer_for_a_resource_that_is_not_there() {
    let router = emulator();

    let endpoint = create_resource(&router, Update::VpcEndpoint, "the endpoint", "first").await;
    let token = "a-token-spent-on-an-endpoint";
    accepted(
        &router,
        Update::VpcEndpoint,
        "the first call",
        update_request(Update::VpcEndpoint, &endpoint, NARROW_SUBNET, token),
    )
    .await;

    // `UpdateVpcEndpoint` models no not-found error, so an id naming no endpoint is the
    // state conflict the endpoint's absence is — and a spent token cannot turn it into a
    // 200 describing another endpoint.
    let unknown = Resource {
        address: json!({ "id": "vpce-00000000000000000" }),
        version: None,
    };
    let (status, code, answer) = call(
        &router,
        "UpdateVpcEndpoint",
        update_request(Update::VpcEndpoint, &unknown, WIDE_SUBNET, token),
    )
    .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ConflictException"),
        "case an id naming no endpoint: expected 400 ConflictException, got {status} {code} with {answer}"
    );

    // The endpoint the token was first spent on is untouched by that refusal.
    let detail = read_one(&router, "the endpoint after the refusal", &text(&endpoint.address, "id"))
        .await;
    let carried = carried_markers(Update::VpcEndpoint, &detail);
    assert!(
        carried.contains(NARROW_SUBNET) && !carried.contains(WIDE_SUBNET),
        "case the endpoint after the refusal: expected only its own change, got {detail}"
    );
}
