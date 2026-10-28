Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 84725303c518fb402f10cf931cddd35e44c00d9a

# The missing-stream 404 builds the stream ARN in the REQUEST's Region instead of the stored cluster's Region

_Filed: 2026-10-28T12:17:57Z_
_Operation: GetStream | Class: error-response_

> _Note: this issue was identified against the code base at commit `84725303c518fb402f10cf931cddd35e44c00d9a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When the addressed cluster IS in the store but the named stream is not, `GetStream` (and, with the identical call, `DeleteStream`) builds the `ResourceNotFoundException` message's stream ARN with `crate::state::request_region(ctx)` — the Region the request was signed for — instead of the Region the stored cluster's own ARN was minted in.

A stream ARN nests under its cluster's ARN (`cluster/{cluster}/stream/{stream}`), so the Region segment of a stream of THAT cluster can only ever be the cluster's. The crate states this rule for itself in `state::resource_region` and applies it at every other site that derives a Region-qualified value for a cluster already in the store (`CreateStream`'s stream ARN, `GetVpcEndpointServiceName`'s service name). These two refusal paths skip the helper, so a cross-Region read of a known cluster reports a 404 naming an ARN that no stream of that cluster could ever have carried (e.g. the cluster lives in `us-east-1`, the message names `arn:aws:dsql:eu-west-1:...:cluster/<c>/stream/<s>`). The crate's own harness builds exactly that topology — one store seen through endpoints signed for two Regions — so the path is reachable, and nothing in the crate justifies the divergence: `state::stream_not_found` carries no comment about the Region it is given, unlike `state::cluster_not_found`, whose doc states the request's Region is correct because no cluster is stored to read one from.

## Evidence

The rule this crate states for itself — `services/service-com-amazonaws-dsql-dsql/src/state.rs:1315-1326`:
"The region a resource ALREADY IN THE STORE lives in: the region of the ARN it was minted with... Every region-qualified value a handler derives FOR such a resource — the ARN of a resource nested under it, a name or an endpoint that fronts it, the KMS key ARN one of its members expands into — is qualified with this region, not with the region the request happens to be signed for... Only the call that MINTS a resource takes the region from the request."

The sibling that obeys it — `src/ops/create_stream.rs:334-342`:
"// A stream is nested UNDER its cluster (`cluster/{cluster}/stream/{stream}`), so its ARN carries the Region the CLUSTER lives in — the Region that cluster's own ARN was minted in (`crate::state::resource_region`) — and not the Region this request happens to be signed for.
            arn: crate::state::stream_arn(
                &crate::state::resource_region(&cluster.arn, &region)," 
and `src/ops/get_vpc_endpoint_service_name.rs:183-187`: "// The endpoint service lives in the CLUSTER's region... let region = crate::state::resource_region(&cluster.arn, &request_region);"

The two sites that break it — `src/ops/get_stream.rs:167` `let region = crate::state::request_region(ctx);` then `src/ops/get_stream.rs:177-183`:
"let Some(stream) = cluster.streams.get(&input.stream_identifier) else {
            return Err(GetStreamErrorModeled::ResourceNotFoundException(
                crate::state::stream_not_found(
                    &region,
                    &input.cluster_identifier,
                    &input.stream_identifier," 
— the stored `cluster` is in scope on the line above — and the identical `src/ops/delete_stream.rs:245-251`.

What the ARN is built from — `src/state.rs:2405-2416`: `pub fn stream_not_found(region: &str, cluster_identifier: &str, stream_identifier: &str) -> ResourceNotFoundException { resource_not_found(&stream_arn(region, cluster_identifier, stream_identifier), ...)` and `src/state.rs:1652`: `message: format!("The resource with ARN {arn} doesn't exist. Verify the ARN and try again.")`.

Contrast the justified sibling — `src/state.rs:2394-2403`: "The modelled `ResourceNotFoundException` for a cluster this service does not hold: [`resource_not_found`] over the ARN the addressed identifier has in the request's own region." No such justification exists for the stream case.

Test coverage: `tests/all/stream_lifecycle.rs:609-670` (`an_unknown_stream_of_a_known_cluster_is_not_found`) asserts only the status and the error code, and runs a single-Region client, so it does not pin the Region in the message; `tests/all/cluster_region_derived_values.rs:6-15` states the rule and exercises it over `harness::Dsql::sharing` for the two derived values it covers (the KMS key ARN and the nested stream ARN at creation), but not for this refusal.

## Steps to Reproduce

1. Build two views of one store signed for different Regions, as `tests/all/cluster_region_derived_values.rs` does with `harness::Dsql::sharing(&engine, "us-east-1")` and `harness::Dsql::sharing(&engine, "eu-west-1")`.
2. Through the us-east-1 view, `CreateCluster` and wait for the clock to settle it; note its ARN is `arn:aws:dsql:us-east-1:<account>:cluster/<cluster>`.
3. Through the eu-west-1 view, call `GET /stream/<cluster>/00000000000000000000000000` (a stream identifier the cluster does not hold).
4. Observe the 404 body: `message` names `arn:aws:dsql:eu-west-1:<account>:cluster/<cluster>/stream/00000000000000000000000000`, an ARN whose Region contradicts the stored cluster's own ARN, instead of the `us-east-1` ARN `state::resource_region` would have produced.
5. Repeat with `DELETE /stream/<cluster>/00000000000000000000000000` for the same refusal from `src/ops/delete_stream.rs:245-251`.
