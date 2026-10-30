// SPDX-License-Identifier: CC0-1.0

//! The Lambda@Edge version rule, held over the configuration THE SERVICE ITSELF handed
//! back: read a stored `DistributionConfig`, put one association into its default cache
//! behaviour, and submit that whole document to `UpdateDistribution` under the version the
//! read reported.
//!
//! `LambdaFunctionAssociation$LambdaFunctionARN` states the rule: "You must specify the ARN
//! of a function version; you can't specify an alias or $LATEST." A function version is
//! addressed by the function's ARN qualified with the NUMBER of a published version, so an
//! association naming anything else is refused with `InvalidLambdaFunctionAssociation`
//! (HTTP 400), the refusal states the version requirement and then names the submitted
//! `LambdaFunctionARN`, and the stored configuration and its version token are untouched.
//!
//! What this suite adds to [`crate::lambda_edge_association`] and
//! [`crate::lambda_edge_association_message`], which submit hand-written minimal
//! configurations: the body here is the FULL document `GetDistributionConfig` answered —
//! every member the service resolved for the caller, a managed cache policy on the default
//! cache behaviour, and the association wrapper the read reports — which is the shape a
//! client that round-trips the config through the API sends. The rule has to hold for that
//! document exactly as it holds for a minimal one.
//!
//! The rows differ along what the rule generalises over: the qualifier form (`$LATEST`, no
//! qualifier at all, an alias name, a number no published version carries), the event type
//! the association fires on, the Region/account/function-name parts of the ARN, and whether
//! the association replaces an association already stored. The accepted rows are the
//! control: a numbered version, of one digit and of several, is stored and read back.

use crate::distribution_lifecycle::{
    call, create_distribution, emulator, Answer, API_VERSION,
};
use crate::lambda_edge_association::{refusal_message, reported_arns, REFUSAL_PREFIX};

/// `GET /<version>/distribution/<id>/config` — the payload IS the `DistributionConfig`
/// document, which is what a round-tripping client resubmits.
async fn get_distribution_config(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        None,
        None,
    )
    .await
}

/// The managed cache policy "Managed-CachingDisabled", named by the id CloudFront publishes
/// for it (`src/state.rs`'s managed catalogue, which the `managed_cache_policy_catalog`
/// suite holds to the published ids).
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The configuration this suite creates its distribution from: one custom origin and a
/// default cache behaviour on a managed cache policy, carrying no association at all.
///
/// Members are written in the order the shapes declare them.
fn base_config(reference: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>origin.example.com</DomainName>\
         <CustomOriginConfig><HTTPPort>80</HTTPPort><HTTPSPort>443</HTTPSPort>\
         <OriginProtocolPolicy>https-only</OriginProtocolPolicy>\
         </CustomOriginConfig></Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>redirect-to-https</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId></DefaultCacheBehavior>\
         <Comment>for {reference}</Comment>\
         <Enabled>true</Enabled></DistributionConfig>"
    )
}

/// The closing tag of the member this suite rewrites, spelled once.
const ASSOCIATIONS_CLOSE: &str = "</LambdaFunctionAssociations>";

/// `config` — a `DistributionConfig` document as the service answered it — with its default
/// cache behaviour's `LambdaFunctionAssociations` replaced by the single association
/// `(arn, event_type)`.
///
/// The whole rest of the document, including every member the service resolved, travels
/// back unchanged, which is what makes this the client's round trip rather than a fresh
/// hand-written body.
fn with_association(config: &str, arn: &str, event_type: &str) -> String {
    let wrapper = format!(
        "<LambdaFunctionAssociations><Quantity>1</Quantity><Items>\
         <LambdaFunctionAssociation><LambdaFunctionARN>{arn}</LambdaFunctionARN>\
         <EventType>{event_type}</EventType><IncludeBody>false</IncludeBody>\
         </LambdaFunctionAssociation></Items></LambdaFunctionAssociations>"
    );
    let open = config
        .find("<DefaultCacheBehavior>")
        .expect("the config read reports a DefaultCacheBehavior");
    let close = open
        + config[open..]
            .find("</DefaultCacheBehavior>")
            .expect("the DefaultCacheBehavior element is closed");
    let behavior = &config[open..close];
    let rewritten = match (
        behavior.find("<LambdaFunctionAssociations>"),
        behavior.find(ASSOCIATIONS_CLOSE),
    ) {
        (Some(start), Some(end)) => format!(
            "{}{wrapper}{}",
            &behavior[..start],
            &behavior[end + ASSOCIATIONS_CLOSE.len()..]
        ),
        // A read that reports no association member at all: state it at the end of the
        // behaviour, which is where the shape declares it relative to the members a read
        // always reports.
        _ => format!("{behavior}{wrapper}"),
    };
    format!("{}{rewritten}{}", &config[..open], &config[close..])
}

/// One association submitted on the round-tripped document.
struct Row {
    case: &'static str,
    arn: &'static str,
    event_type: &'static str,
}

/// A function ARN with no qualifier: it addresses the function, not one of its versions.
const UNQUALIFIED: &str = "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip";

/// The rows whose association names no published version, so every one of them is refused.
fn refused_rows() -> Vec<Row> {
    vec![
        Row {
            case: "latest-qualifier",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:$LATEST",
            event_type: "viewer-request",
        },
        Row {
            // The same unpublished qualifier on another event type: the rule is about the
            // ARN, not about when the function would fire.
            case: "latest-qualifier-on-origin-request",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:$LATEST",
            event_type: "origin-request",
        },
        Row {
            case: "unqualified-function-arn",
            arn: UNQUALIFIED,
            event_type: "viewer-response",
        },
        Row {
            case: "alias-qualifier",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:live",
            event_type: "origin-response",
        },
        Row {
            // Published versions count from 1, so no function carries a version 0 or one
            // written with a leading zero.
            case: "version-zero",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:0",
            event_type: "viewer-request",
        },
        Row {
            case: "version-leading-zero",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:01",
            event_type: "origin-request",
        },
        Row {
            // Another Region, account and function-name form, still unpublished: the
            // message names the string the request wrote, wherever the function lives.
            case: "other-region-account-and-name-form",
            arn: "arn:aws:lambda:eu-west-3:210987654321:function:edge.round_trip-2:$LATEST",
            event_type: "viewer-response",
        },
    ]
}

/// The rows whose association names a published version, which is what CloudFront stores.
fn accepted_rows() -> Vec<Row> {
    vec![
        Row {
            case: "version-1",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:1",
            event_type: "viewer-request",
        },
        Row {
            // A version number is however many digits the function has versions.
            case: "version-many-digits",
            arn: "arn:aws:lambda:us-east-1:000000000000:function:edge-round-trip:1042",
            event_type: "origin-response",
        },
    ]
}

/// A refused write: HTTP 400, the modelled code, and the version requirement followed by
/// the submitted ARN.
fn assert_refusal(answer: &Answer, case: &str, arn: &str) {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {}",
        answer.status, answer.raw
    );
    assert_eq!(
        answer.error_code(),
        "InvalidLambdaFunctionAssociation",
        "case {case}: expected InvalidLambdaFunctionAssociation, got {:?} with {}",
        answer.error_code(),
        answer.raw
    );
    let message = answer
        .root()
        .find_text("Message")
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        message,
        refusal_message(arn),
        "case {case}: expected the refusal to state the version requirement and then the \
         submitted ARN, got {message:?}"
    );
    assert!(
        message.starts_with(REFUSAL_PREFIX),
        "case {case}: expected the fixed sentence first, got {message:?}"
    );
}

/// Update `id` with the stored configuration carrying `(arn, event_type)`, under `etag`.
async fn update_with_association(
    router: &smithy_server_runtime::MultiServiceRouter,
    id: &str,
    etag: &str,
    config: &str,
    arn: &str,
    event_type: &str,
) -> Answer {
    let body = with_association(config, arn, event_type);
    assert_eq!(
        body.matches("<LambdaFunctionARN>").count(),
        1,
        "the submitted document carries exactly one association, got {body}"
    );
    call(
        router,
        "PUT",
        &format!("/{API_VERSION}/distribution/{id}/config"),
        Some(&body),
        Some(etag),
    )
    .await
}

#[tokio::test]
async fn the_version_rule_holds_for_the_config_the_service_handed_back() {
    let router = emulator();

    let created = create_distribution(&router, &base_config("ref-round-trip")).await;
    assert_eq!(
        created.status, 201,
        "case setup: expected HTTP 201, got {} with {}",
        created.status, created.raw
    );
    let id = created.root().text_at("Id").unwrap_or_default().to_string();

    let read = get_distribution_config(&router, &id).await;
    assert_eq!(
        read.status, 200,
        "case setup-config-read: expected HTTP 200, got {} with {}",
        read.status, read.raw
    );
    let stored_config = read.raw.clone();
    let stored_etag = read.header("etag").to_string();
    assert!(
        reported_arns(&read).is_empty(),
        "case setup-config-read: the distribution starts with no association, got {:?}",
        reported_arns(&read)
    );

    // Every unpublished form is refused on the document the read answered, and each
    // refusal leaves the stored configuration and its version exactly where they were.
    for row in refused_rows() {
        let answer = update_with_association(
            &router,
            &id,
            &stored_etag,
            &stored_config,
            row.arn,
            row.event_type,
        )
        .await;
        assert_refusal(&answer, row.case, row.arn);

        let after = get_distribution_config(&router, &id).await;
        assert_eq!(
            after.raw, stored_config,
            "case {}: a refused update must leave the stored configuration unchanged, got \
             {}",
            row.case, after.raw
        );
        assert_eq!(
            after.header("etag"),
            stored_etag,
            "case {}: a refused update mints no new version, got {:?}",
            row.case,
            after.header("etag")
        );
    }

    // The control: a published version IS stored, and the read back reports it. Each
    // accepted row starts from the configuration the previous one left behind, under the
    // version that write handed back.
    let mut config = stored_config;
    let mut etag = stored_etag;
    for row in accepted_rows() {
        let accepted =
            update_with_association(&router, &id, &etag, &config, row.arn, row.event_type).await;
        assert_eq!(
            accepted.status, 200,
            "case {}: expected HTTP 200, got {} with {}",
            row.case, accepted.status, accepted.raw
        );
        assert_eq!(
            reported_arns(&accepted),
            vec![row.arn.to_string()],
            "case {}: expected the answer to carry the submitted association, got {:?}",
            row.case,
            reported_arns(&accepted)
        );

        let after = get_distribution_config(&router, &id).await;
        assert_eq!(
            after.status, 200,
            "case {}: expected the config read to succeed, got {} with {}",
            row.case, after.status, after.raw
        );
        assert_eq!(
            reported_arns(&after),
            vec![row.arn.to_string()],
            "case {}: expected the stored configuration to name {:?}, got {:?}",
            row.case,
            row.arn,
            reported_arns(&after)
        );
        config = after.raw.clone();
        etag = after.header("etag").to_string();
    }

    // And a refusal still changes nothing once an association IS stored: the last accepted
    // version stays associated after an update that tries to move it onto an unpublished
    // one.
    let stored_arn = accepted_rows()
        .last()
        .map(|row| row.arn.to_string())
        .expect("the accepted rows are not empty");
    let refused =
        update_with_association(&router, &id, &etag, &config, UNQUALIFIED, "viewer-request").await;
    assert_refusal(&refused, "replace-a-stored-version", UNQUALIFIED);
    let after = get_distribution_config(&router, &id).await;
    assert_eq!(
        reported_arns(&after),
        vec![stored_arn.clone()],
        "case replace-a-stored-version: expected the stored association to remain \
         {stored_arn:?}, got {:?}",
        reported_arns(&after)
    );
    assert_eq!(
        after.header("etag"),
        etag,
        "case replace-a-stored-version: a refused update mints no new version, got {:?}",
        after.header("etag")
    );
}
