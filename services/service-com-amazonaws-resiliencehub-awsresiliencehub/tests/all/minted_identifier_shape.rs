// SPDX-License-Identifier: CC0-1.0

//! The SHAPE of every identifier this service mints itself, end to end over HTTP.
//!
//! The rule under test: an identifier this service generates rather than taking from
//! the request — the `resolutionId` of a resource resolution and the id segment of a
//! minted `appArn` or `policyArn` — is an opaque, randomly generated version-4 UUID:
//! canonical lowercase `8-4-4-4-12` text with the version and variant bits RFC 4122
//! gives a random UUID, and no part of it encodes the minting instant, a sequence
//! number or anything else a client could read an ordering or another resource's id
//! out of.
//!
//! The harness (`emulator`, `post`, `Reply`) is the one `application_lifecycle`
//! drives every request of this crate's suites through: the real wire path,
//! `MultiServiceRouter::dispatch` with a restJson1 request.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_resiliencehub_awsresiliencehub::state::DRAFT_APP_VERSION;
use smithy_server_runtime::MultiServiceRouter;
use uuid::{Uuid, Variant, Version};

use super::application_lifecycle::{create_a_policy, emulator, post, ACCOUNT, REGION};

/// The modelled `@length` of `String255`, the shape `resolutionId` targets
/// (`smithy_query query="operation" operation="ResolveAppVersionResources"`:
/// `resolutionId` targets `String255`, length 1..255).
const RESOLUTION_ID_LENGTH: std::ops::RangeInclusive<usize> = 1..=255;

/// Which minting path an identifier came out of. The rows of the table below differ
/// along this: every operation of this service that generates an identifier instead
/// of echoing one, and both kinds of app version a resolution can be started for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Minted {
    /// The id segment of the `appArn` `CreateApp` mints.
    AppArnId,
    /// The id segment of the `policyArn` `CreateResiliencyPolicy` mints.
    PolicyArnId,
    /// The `resolutionId` of a resolution of the DRAFT app version.
    DraftResolution,
    /// The `resolutionId` of a resolution of a PUBLISHED app version.
    PublishedResolution,
}

/// One minted identifier, with the case name every assertion about it reports.
#[derive(Debug)]
struct Identifier {
    case: String,
    kind: Minted,
    text: String,
}

/// Create an application and answer its `appArn`.
async fn create_an_app(router: &MultiServiceRouter, name: &str) -> String {
    let reply = post(router, "/create-app", &json!({"name": name})).await;
    reply.ok(name)["app"]["appArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: a minted appArn, got {:?}", reply.body))
        .to_string()
}

/// Add one physical resource to `app_arn`'s draft version, so the draft holds
/// something and can be published.
async fn add_a_resource(router: &MultiServiceRouter, app_arn: &str, identifier: &str) {
    let reply = post(
        router,
        "/create-app-version-resource",
        &json!({
            "appArn": app_arn,
            "resourceName": identifier,
            "logicalResourceId": {"identifier": identifier},
            "physicalResourceId": identifier,
            "resourceType": "AWS::SQS::Queue",
            "appComponents": ["queue-tier"],
        }),
    )
    .await;
    reply.ok(identifier);
}

/// Publish `app_arn`'s draft and answer the `appVersion` the publish minted.
async fn publish(router: &MultiServiceRouter, app_arn: &str) -> String {
    let reply = post(router, "/publish-app-version", &json!({"appArn": app_arn})).await;
    reply.ok("publish")["appVersion"]
        .as_str()
        .unwrap_or_else(|| panic!("a published appVersion, got {:?}", reply.body))
        .to_string()
}

/// Start a resolution of `app_version` of `app_arn` and answer its `resolutionId`.
async fn resolve(router: &MultiServiceRouter, app_arn: &str, app_version: &str) -> String {
    let reply = post(
        router,
        "/resolve-app-version-resources",
        &json!({"appArn": app_arn, "appVersion": app_version}),
    )
    .await;
    reply.ok(app_version)["resolutionId"]
        .as_str()
        .unwrap_or_else(|| panic!("a minted resolutionId, got {:?}", reply.body))
        .to_string()
}

/// The id segment of an ARN this service minted: everything after
/// `arn:aws:resiliencehub:<region>:<account>:<kind>/`.
fn arn_id_segment(case: &str, arn: &str, kind: &str) -> String {
    let prefix = format!("arn:aws:resiliencehub:{REGION}:{ACCOUNT}:{kind}/");
    arn.strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("case {case:?}: expected an ARN starting {prefix:?}, got {arn:?}"))
        .to_string()
}

/// Mint identifiers through every path, INTERLEAVED across the paths and repeated,
/// in the order the service minted them — so an id carrying a shared sequence
/// number or a shared clock prefix is visible as structure between neighbours.
async fn minted_identifiers(router: &MultiServiceRouter, rounds: usize) -> Vec<Identifier> {
    let mut minted = Vec::new();
    for round in 0..rounds {
        let app_arn = create_an_app(router, &format!("minted-ids-{round}")).await;
        minted.push(Identifier {
            case: format!("the appArn id of application {round}"),
            kind: Minted::AppArnId,
            text: arn_id_segment("a minted appArn", &app_arn, "app"),
        });

        let policy_arn = create_a_policy(router, &format!("minted-ids-policy-{round}")).await;
        minted.push(Identifier {
            case: format!("the policyArn id of policy {round}"),
            kind: Minted::PolicyArnId,
            text: arn_id_segment("a minted policyArn", &policy_arn, "resiliency-policy"),
        });

        minted.push(Identifier {
            case: format!("the resolutionId of the draft of application {round}"),
            kind: Minted::DraftResolution,
            text: resolve(router, &app_arn, DRAFT_APP_VERSION).await,
        });

        add_a_resource(router, &app_arn, &format!("orders-queue-{round}")).await;
        let published = publish(router, &app_arn).await;
        minted.push(Identifier {
            case: format!("the resolutionId of published version {published} of app {round}"),
            kind: Minted::PublishedResolution,
            text: resolve(router, &app_arn, &published).await,
        });
    }
    minted
}

/// The last `12` hex digits of a canonical UUID, as a number — where the pre-fix
/// implementation carried a process-wide mint counter, so two neighbours differing
/// by one here is exactly the structure the rule forbids.
fn trailing_group(id: &str) -> u64 {
    let group = id.rsplit('-').next().expect("a hyphenated UUID");
    u64::from_str_radix(group, 16).unwrap_or_else(|_| panic!("hex in the last group of {id:?}"))
}

/// The leading `8` hex digits of a canonical UUID — where the pre-fix
/// implementation carried the high bits of the minting instant, shared by every id
/// minted within the same few seconds.
fn leading_group(id: &str) -> &str {
    id.split('-').next().expect("a hyphenated UUID")
}

/// Every identifier the service mints is a canonically shaped, randomly generated
/// version-4 UUID — whichever operation minted it.
#[tokio::test]
async fn every_minted_identifier_is_a_canonical_version_4_uuid() {
    let router = emulator();
    let minted = minted_identifiers(&router, 4).await;

    for kind in [
        Minted::AppArnId,
        Minted::PolicyArnId,
        Minted::DraftResolution,
        Minted::PublishedResolution,
    ] {
        assert!(
            minted.iter().any(|id| id.kind == kind),
            "the table must cover {kind:?}, got {minted:?}"
        );
    }

    for Identifier { case, kind, text } in &minted {
        let parsed = Uuid::parse_str(text).unwrap_or_else(|error| {
            panic!("case {case:?}: expected a UUID, got {text:?} ({error})")
        });
        assert_eq!(
            parsed.get_version(),
            Some(Version::Random),
            "case {case:?}: a server-generated {kind:?} must be a randomly generated \
             (version-4) UUID, got {text:?} with version {:?}",
            parsed.get_version_num()
        );
        assert_eq!(
            parsed.get_variant(),
            Variant::RFC4122,
            "case {case:?}: a server-generated {kind:?} must carry the RFC-4122 variant \
             bits, got {text:?} with variant {:?}",
            parsed.get_variant()
        );
        // The WIRE text is already the canonical form, so a client that parses and
        // re-renders the value gets back exactly what it was handed.
        assert_eq!(
            &parsed.hyphenated().to_string(),
            text,
            "case {case:?}: the minted {kind:?} must be canonical lowercase \
             hyphenated text, got {text:?}"
        );
        assert!(
            RESOLUTION_ID_LENGTH.contains(&text.chars().count()),
            "case {case:?}: a minted {kind:?} must satisfy String255's modelled length \
             {RESOLUTION_ID_LENGTH:?}, got {} characters ({text:?})",
            text.chars().count()
        );
    }
}

/// The minted identifiers carry no structure a client can read: they are all
/// distinct, no neighbour is one step from the next, none shares a prefix with
/// another, and their minting order is not their sort order.
#[tokio::test]
async fn minted_identifiers_share_no_derivable_structure() {
    let router = emulator();
    let minted = minted_identifiers(&router, 4).await;

    let distinct: BTreeSet<&str> = minted.iter().map(|id| id.text.as_str()).collect();
    assert_eq!(
        distinct.len(),
        minted.len(),
        "every minted identifier must be unique, got {minted:?}"
    );

    for pair in minted.windows(2) {
        let (earlier, later) = (&pair[0], &pair[1]);
        let case = format!("{} then {}", earlier.case, later.case);
        let step = trailing_group(&later.text).abs_diff(trailing_group(&earlier.text));
        assert!(
            step > 1,
            "case {case:?}: consecutively minted ids must not be one step apart — no \
             sequence number on the wire — got {:?} then {:?}",
            earlier.text,
            later.text
        );
    }

    let leading: BTreeSet<&str> = minted.iter().map(|id| leading_group(&id.text)).collect();
    assert_eq!(
        leading.len(),
        minted.len(),
        "no two minted ids may share a leading group — a shared prefix is a clock or a \
         namespace a client can read — got {:?}",
        minted
            .iter()
            .map(|id| id.text.clone())
            .collect::<Vec<String>>()
    );

    let in_minting_order: Vec<&str> = minted.iter().map(|id| id.text.as_str()).collect();
    let sorted: Vec<&str> = distinct.iter().copied().collect();
    assert_ne!(
        in_minting_order, sorted,
        "minted ids must not come out in sort order — that is a clock or a counter in \
         the high bits"
    );
}

/// A randomly shaped id is still the id that ADDRESSES the resolution: the value
/// `ResolveAppVersionResources` hands back is what
/// `DescribeAppVersionResourcesResolutionStatus` reads the resolution by.
#[tokio::test]
async fn a_minted_resolution_id_still_addresses_its_resolution() {
    let router = emulator();
    let app_arn = create_an_app(&router, "minted-id-roundtrip").await;
    add_a_resource(&router, &app_arn, "orders-queue").await;

    let published = publish(&router, &app_arn).await;
    for app_version in [DRAFT_APP_VERSION.to_string(), published] {
        let resolution_id = resolve(&router, &app_arn, &app_version).await;
        let described = post(
            &router,
            "/describe-app-version-resources-resolution-status",
            &json!({
                "appArn": app_arn,
                "appVersion": app_version,
                "resolutionId": resolution_id,
            }),
        )
        .await;
        assert_eq!(
            described.ok(&app_version)["resolutionId"],
            json!(resolution_id),
            "case {app_version:?}: the minted resolutionId must read back the resolution \
             it was minted for, got {:?}",
            described.body
        );
    }
}
