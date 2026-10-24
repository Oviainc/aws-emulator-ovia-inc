// SPDX-License-Identifier: CC0-1.0

//! The `Location` header this service reports for a resource it has just created, end to
//! end over HTTP.
//!
//! ONE rule, over every create on this surface that binds an output member to the
//! `Location` header: the header is the new resource's own path under this service's
//! endpoint, terminated by the identifier the SERVICE generated for that resource — the
//! value the same response reports for it — and NEVER by a value the request supplied (a
//! name, a caller reference). Which generated identifier a resource is addressed by is
//! the one its own API uses: an `Id` token for a distribution, an invalidation, an origin
//! access identity and a response headers policy, and the ARN for a key value store,
//! which is how the whole key value store surface names a store (`KeyValueStore$ARN` on
//! the control plane, the `KvsARN` label of every `cloudfront-keyvaluestore` data-plane
//! route). Either way the header's last segment is the generated id: a store's ARN ends
//! with it.
//!
//! The rows span what the rule generalises over: which create is called (five of them,
//! minting ids in their own shapes — `E`-prefixed tokens, `I`-prefixed tokens and
//! UUIDs), which identifier form the resource is addressed by, whether the create's route
//! is nested under another resource, and which caller-supplied value the request carried
//! (a store name, a policy name, a caller reference). The key value store create the
//! issue was filed about is five of the rows.

use crate::distribution_lifecycle::{
    batch_xml, call, config, config_table, create_distribution, create_invalidation, emulator,
    Answer, API_VERSION, HOST,
};
use crate::key_value_store_lifecycle::{create_key_value_store, create_table, member};

/// One create under test, and what it answered.
struct Case {
    /// Names the row in every assertion below.
    case: String,
    /// The collection path the new resource lives under, between the API version and the
    /// identifier.
    collection: String,
    /// The member of the created resource's payload that carries the identifier the
    /// `Location` must address it by: `ARN` for a key value store, `Id` for the rest.
    identifier_member: &'static str,
    /// A value the REQUEST supplied that the `Location` must not address the resource by.
    caller_input: String,
    /// Whether the `Location` path is also the route the resource is read back on. It is
    /// for every create here except `CreateKeyValueStore`, whose read
    /// (`DescribeKeyValueStore`, `GET /2020-05-31/key-value-store/{Name}`) is addressed
    /// by name — the model's own asymmetry, not this header's.
    readable: bool,
    answer: Answer,
}

/// A minimal `CloudFrontOriginAccessIdentityConfig`: both members are required on the
/// wire, so a request carries both.
fn origin_access_identity_xml(reference: &str) -> String {
    format!(
        "<CloudFrontOriginAccessIdentityConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Comment>location rule</Comment>\
         </CloudFrontOriginAccessIdentityConfig>"
    )
}

/// A minimal `ResponseHeadersPolicyConfig`: `Name` is its only required member.
fn response_headers_policy_xml(name: &str) -> String {
    format!(
        "<ResponseHeadersPolicyConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         </ResponseHeadersPolicyConfig>"
    )
}

/// Every create this service answers with a `Location`, issued once each, over one
/// emulator.
async fn location_cases(router: &smithy_server_runtime::MultiServiceRouter) -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();

    // CreateKeyValueStore: the store is addressed by its ARN, and the request carries a
    // caller-chosen `Name`. The rows are the whole key value store table, so the name's
    // length, character classes and the presence of a comment or tags all vary.
    for create in create_table() {
        let answer = create_key_value_store(router, &create.xml()).await;
        cases.push(Case {
            case: format!("CreateKeyValueStore: {}", create.case),
            collection: "key-value-store".to_string(),
            identifier_member: "ARN",
            caller_input: create.name.clone(),
            readable: false,
            answer,
        });
    }

    // CreateDistribution: addressed by an `E`-prefixed id token, with a caller-chosen
    // `CallerReference` in the request.
    for config in config_table() {
        let answer = create_distribution(router, &config.xml()).await;
        cases.push(Case {
            case: format!("CreateDistribution: {}", config.case),
            collection: "distribution".to_string(),
            identifier_member: "Id",
            caller_input: config.caller_reference.clone(),
            readable: true,
            answer,
        });
    }

    // CreateCloudFrontOriginAccessIdentity: an `E`-prefixed id under a two-segment
    // collection path.
    for reference in ["oai-location-one", "oai-location-two"] {
        let answer = call(
            router,
            "POST",
            &format!("/{API_VERSION}/origin-access-identity/cloudfront"),
            Some(&origin_access_identity_xml(reference)),
            None,
        )
        .await;
        cases.push(Case {
            case: format!("CreateCloudFrontOriginAccessIdentity: {reference}"),
            collection: "origin-access-identity/cloudfront".to_string(),
            identifier_member: "Id",
            caller_input: reference.to_string(),
            readable: true,
            answer,
        });
    }

    // CreateResponseHeadersPolicy: a minted UUID id, and a caller-chosen `Name` the
    // policy is NOT addressed by.
    for name in ["policy-location-one", "policy-location-two"] {
        let answer = call(
            router,
            "POST",
            &format!("/{API_VERSION}/response-headers-policy"),
            Some(&response_headers_policy_xml(name)),
            None,
        )
        .await;
        cases.push(Case {
            case: format!("CreateResponseHeadersPolicy: {name}"),
            collection: "response-headers-policy".to_string(),
            identifier_member: "Id",
            caller_input: name.to_string(),
            readable: true,
            answer,
        });
    }

    // CreateInvalidation: an `I`-prefixed id under a collection NESTED in the
    // distribution it belongs to, with a caller-chosen `CallerReference`.
    let host_config = config("the distribution an invalidation belongs to", "ref-location-host");
    let host = create_distribution(router, &host_config.xml()).await;
    let distribution_id = member(host.root(), "Id", "the host distribution", &host.raw);
    for (case, reference, paths) in [
        ("one path", "invalidation-location-one", vec!["/index.html"]),
        (
            "two paths",
            "invalidation-location-two",
            vec!["/a/*", "/b.css"],
        ),
    ] {
        let answer =
            create_invalidation(router, &distribution_id, &batch_xml(reference, &paths)).await;
        cases.push(Case {
            case: format!("CreateInvalidation: {case}"),
            collection: format!("distribution/{distribution_id}/invalidation"),
            identifier_member: "Id",
            caller_input: reference.to_string(),
            readable: true,
            answer,
        });
    }

    cases
}

/// Every create reports the new resource's `Location` as that resource's own path under
/// this service's endpoint, terminated by the identifier the service generated for it and
/// reported in the same response — never by a value the request supplied. The header's
/// last segment is always the generated id, and where the path is also the resource's
/// read route, following it reaches the resource that was just created.
#[tokio::test]
async fn a_create_addresses_the_new_resource_by_its_generated_identifier() {
    let router = emulator();

    for Case {
        case,
        collection,
        identifier_member,
        caller_input,
        readable,
        answer,
    } in location_cases(&router).await
    {
        assert_eq!(
            answer.status, 201,
            "case {case}: expected HTTP 201, got {} with {}",
            answer.status, answer.raw
        );

        let id = member(answer.root(), "Id", &case, &answer.raw);
        let identifier = member(answer.root(), identifier_member, &case, &answer.raw);
        let location = answer.header("location").to_string();
        assert_eq!(
            location,
            format!("https://{HOST}/{API_VERSION}/{collection}/{identifier}"),
            "case {case}: expected the new resource's path terminated by its generated \
             {identifier_member} {identifier:?}, got {location:?}"
        );

        // The identifier an ARN carries IS the generated id, so whichever form the
        // resource is addressed by, the header ends at that id — and never at caller
        // input.
        assert!(
            location.ends_with(&format!("/{id}")),
            "case {case}: expected Location to end with the generated Id {id:?}, got \
             {location:?}"
        );
        let segment = location.rsplit('/').next().unwrap_or_default().to_string();
        assert_ne!(
            segment, caller_input,
            "case {case}: Location must not address the resource by the request's own \
             value {caller_input:?}"
        );

        // And where the resource's read route is that path, following Location reaches
        // the resource the create just made.
        if readable {
            let path = location
                .strip_prefix(&format!("https://{HOST}"))
                .unwrap_or_else(|| {
                    panic!("case {case}: Location is not an absolute URI: {location:?}")
                });
            let read = call(&router, "GET", path, None, None).await;
            assert_eq!(
                read.status, 200,
                "case {case}: following Location {location:?} must reach the resource, got {} \
                 with {}",
                read.status, read.raw
            );
            let read_id = member(read.root(), "Id", &case, &read.raw);
            assert_eq!(
                read_id, id,
                "case {case}: following Location must reach the created resource, got {read_id:?}"
            );
        }
    }
}
