// SPDX-License-Identifier: CC0-1.0

//! The Amazon Keyspaces table a `cassandra` scalable target names, resolved in the service
//! that owns it.
//!
//! A scalable target is a capacity dimension OF A RESOURCE, so registering one has to resolve
//! that resource where it lives: `cassandra:table:ReadCapacityUnits` and
//! `cassandra:table:WriteCapacityUnits` scale the provisioned throughput of ONE Amazon
//! Keyspaces table, so a table that does not exist has no throughput to scale, and neither
//! has one whose capacity mode provisions none — "throughputMode:PAY_PER_REQUEST" is the
//! on-demand mode, where there is no provisioned value for a policy to move. That is the
//! same rule the DynamoDB and ECS resolvers apply one namespace over
//! ([`super::resolve_dynamodb_capacity`], [`super::resolve_ecs_capacity`]); only WHERE the
//! answer comes from differs.
//!
//! ## Why this one is asked over the wire
//!
//! Every other resolver reads the owning service's store through its state TYPE. Amazon
//! Keyspaces cannot be read that way from here: that service's own auto scaling is
//! implemented ON TOP of this one, so its crate already depends on this crate, and naming its
//! state type here would be a dependency cycle. So the table is resolved by CALLING the
//! owning service — `GetTable`, the one operation that reports a table's capacity
//! specification — in-process over the callback transport, signed the way every other
//! in-process caller signs ([`::service_common::in_process_sigv4`]). The answer is that
//! service's own, so the two can no more disagree than two readers of one store can.
//!
//! An engine on which Amazon Keyspaces is not mounted answers the call with the router's
//! `UnknownService`, which resolves NOTHING rather than refusing: the same answer this
//! service gives for a namespace whose owning store it cannot reach.

use ::serde_json::Value;

/// Amazon Keyspaces' signing name, which is what routes the in-process call to its own
/// handlers, and the service shape an awsJson `X-Amz-Target` names an operation under (that
/// model's `sigv4Name: cassandra` and service shape `KeyspacesService`,
/// `smithy_query(query="service_info")`). It speaks `awsJson1_0`.
const KEYSPACES_SIGNING_NAME: &str = "cassandra";
/// See [`KEYSPACES_SIGNING_NAME`].
const KEYSPACES_TARGET_PREFIX: &str = "KeyspacesService";
/// The media type an `awsJson1_0` request and its answer carry.
const AWS_JSON_1_0: &str = "application/x-amz-json-1.0";

/// The error code Amazon Keyspaces answers for a table it does not hold
/// (`ResourceNotFoundException`, the modelled 404 of `GetTable`).
const TABLE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The capacity mode that HAS provisioned throughput: "throughputMode:PROVISIONED ... you
/// specify the number of reads and writes per second that you need for your application"
/// (`CapacitySpecification$throughputMode`). Any other mode provisions none.
const PROVISIONED_THROUGHPUT_MODE: &str = "PROVISIONED";

/// The keyspace and table a `cassandra` `ResourceId` names:
/// `keyspace/<keyspaceName>/table/<tableName>`, the form Application Auto Scaling documents
/// for that namespace's table resource type.
pub fn split_cassandra_resource_id(resource_id: &str) -> Option<(&str, &str)> {
    let rest = resource_id.strip_prefix("keyspace/")?;
    let (keyspace, table) = rest.split_once("/table/")?;
    (!keyspace.is_empty() && !table.is_empty() && !table.contains('/'))
        .then_some((keyspace, table))
}

/// Resolve the table a `cassandra` scalable target names, in Amazon Keyspaces itself.
///
/// * `Ok(())` — the table exists with provisioned throughput, or this emulator could not
///   reach the owning service at all, which resolves nothing rather than refusing;
/// * `Err(message)` — the table cannot carry this target, as the `ValidationException`
///   message naming why.
pub async fn resolve_cassandra_table(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource_id: &str,
    scalable_dimension: &str,
) -> Result<(), String> {
    let Some((keyspace, table)) = split_cassandra_resource_id(resource_id) else {
        return Err(format!(
            "ResourceId not valid for the cassandra service namespace: {resource_id} must name \
             an Amazon Keyspaces table as keyspace/<keyspace>/table/<table>"
        ));
    };
    let request = ::serde_json::json!({"keyspaceName": keyspace, "tableName": table});
    match get_table(ctx, region, &request).await {
        // The table resolved: what decides the registration is the capacity mode it is in.
        // A mode the answer does not report is one this reader cannot judge, so it resolves
        // nothing rather than refusing. The refusal names the TABLE and the mode its
        // throughput must be in for a target to scale it.
        Ok(answered) => match reported_throughput_mode(&answered) {
            None | Some(PROVISIONED_THROUGHPUT_MODE) => Ok(()),
            Some(_) => Err(super::scalable_target_validation_failed(&format!(
                "Table {table} must be in {PROVISIONED_THROUGHPUT_MODE} throughput mode"
            ))),
        },
        Err(refusal) if refusal.code == TABLE_NOT_FOUND => {
            // The refusal names the resource and the dimension the registration asked for,
            // and states as its reason which table was looked for in which keyspace.
            Err(super::resource_validation_failed(
                resource_id,
                scalable_dimension,
                &format!("Table {table} does not exist in keyspace {keyspace}"),
            ))
        }
        // Anything else is a fact about this process rather than about the request — the
        // owning service is not mounted on this engine, or its answer could not be read — so
        // the resource is UNRESOLVED and the registration is answered on its own terms.
        Err(_) => Ok(()),
    }
}

/// One refusal another emulated service answered: the code a client branches on.
struct Refused {
    code: String,
}

/// The capacity mode `GetTable` reports for the table it answered
/// (`capacitySpecification.throughputMode`), when it reports one.
fn reported_throughput_mode(answered: &Value) -> Option<&str> {
    answered
        .get("capacitySpecification")
        .and_then(|capacity| capacity.get("throughputMode"))
        .and_then(Value::as_str)
        .filter(|mode| !mode.is_empty())
}

/// Call Amazon Keyspaces' `GetTable` in-process and read the table it reports.
async fn get_table(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    request: &Value,
) -> Result<Value, Refused> {
    let unreachable = || Refused {
        code: "InternalFailure".to_string(),
    };
    let body = ::serde_json::to_vec(request).map_err(|_| unreachable())?;
    let mut call = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            "host",
            format!("{KEYSPACES_SIGNING_NAME}.{region}.amazonaws.com"),
        )
        .header("content-type", AWS_JSON_1_0)
        .header("x-amz-target", format!("{KEYSPACES_TARGET_PREFIX}.GetTable"))
        .body(body)
        .map_err(|_| unreachable())?;
    // The emulator verifies every request signature, so an in-process call carries one too:
    // the credential the framework hands this handler, signed through the tree's one
    // in-process signer.
    ::service_common::in_process_sigv4::sign_in_process_request(
        &mut call,
        KEYSPACES_SIGNING_NAME,
        region,
        ctx,
    )
    .await
    .map_err(|_| unreachable())?;
    let (parts, body) = call.into_parts();
    let call = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let call = ::aws_smithy_runtime_api::http::Request::try_from(call).map_err(|_| unreachable())?;
    let connector = ctx.callback().http_connector();
    let answer = {
        use ::aws_smithy_runtime_api::client::http::HttpConnector;
        HttpConnector::call(&connector, call)
            .await
            .map_err(|_| unreachable())?
    };
    let status = answer.status().as_u16();
    let payload = answer.body().bytes().ok_or_else(unreachable)?.to_vec();
    let document: Value = ::serde_json::from_slice(&payload).unwrap_or(Value::Null);
    if (200..300).contains(&status) {
        return Ok(document);
    }
    // An awsJson error names its shape in `__type` (or in the `x-amzn-errortype` header,
    // which this reader does not need): the code is the last segment of that shape id.
    let code = document
        .get("__type")
        .and_then(Value::as_str)
        .and_then(|shape| shape.rsplit(['#', '/']).next())
        .unwrap_or_default()
        .to_string();
    Err(Refused { code })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a `cassandra` resource id names ONE table inside ONE keyspace, as
    /// `keyspace/<keyspace>/table/<table>`; anything else names no table at all.
    #[test]
    fn a_cassandra_resource_id_names_one_table_of_one_keyspace() {
        /// (case, the resource id, the keyspace and table it names)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "a table of a keyspace",
                "keyspace/orders/table/items",
                Some(("orders", "items")),
            ),
            (
                "names underscored the way Amazon Keyspaces allows",
                "keyspace/my_ks/table/my_table",
                Some(("my_ks", "my_table")),
            ),
            ("no table segment", "keyspace/orders", None),
            ("an empty table name", "keyspace/orders/table/", None),
            ("an empty keyspace name", "keyspace//table/items", None),
            ("a further segment", "keyspace/orders/table/items/extra", None),
            ("a DynamoDB resource id", "table/orders", None),
            ("nothing at all", "", None),
        ];
        for (case, resource_id, expected) in rows {
            let actual = split_cassandra_resource_id(resource_id);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
