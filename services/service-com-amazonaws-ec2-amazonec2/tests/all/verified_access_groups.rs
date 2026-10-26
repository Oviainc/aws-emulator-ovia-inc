// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access GROUP slice: `CreateVerifiedAccessGroup`, driven
//! over the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVerifiedAccessGroup` MINTS A GROUP ON THE INSTANCE THE REQUEST NAMES: every
//!    member the request sets round-trips through the response and the store, and the members
//!    the service owns (the group id, its ARN, the owning account, the timestamps) are
//!    derived rather than taken;
//! 2. EVERY EC2 RESOURCE THE REQUEST REFERENCES MUST BE ONE THE ACCOUNT HOLDS: an instance id
//!    that resolves to nothing is refused at HTTP 400 with that resource's own
//!    `Invalid<Kind>.NotFound` code, naming the id the request gave, and NOTHING is stored;
//! 3. the group is the only resource this call creates, so a tag specification naming another
//!    resource type is refused, and the published tag-count and client-token bounds hold;
//! 4. one `ClientToken` names one group, and a `DryRun` reports the outcome the request would
//!    have had and WRITES NOTHING;
//! 5. the group is in the STORE rather than echoed: `CreateVerifiedAccessEndpoint` resolves
//!    it and reports the instance read off it, and `DescribeVerifiedAccessEndpoints` narrows
//!    by it.
//!
//! Each table row is a distinct input along what the rule generalises over — which optional
//! members the request carries, which instance it names, which reference is unresolvable and
//! in which form, which resource type a tag specification names, how many tags, how long a
//! client token is — so no row is the specification.
//!
//! The instances the groups hang off are seeded through
//! [`ec2::state::ComAmazonawsEc2Amazonec2State::insert_verified_access_instance`], the writer
//! seam `CreateVerifiedAccessInstance` lands on too — the way the subnet and image suites
//! seed through their own insert seams.

use ec2::ops::types::{VerifiedAccessGroup, VerifiedAccessInstance};
use ec2::state::{
    availability_zones_in_region, ComAmazonawsEc2Amazonec2State as Ec2State, SecurityGroupSpec,
    SubnetSpec, VerifiedAccessGroupRecord, MAX_CLIENT_TOKEN_CHARS, MAX_TAGS_PER_RESOURCE,
    MAX_VERIFIED_ACCESS_GROUPS,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};

/// The Region every call here is signed for: the group's ARN names it, so a Region that is
/// not the emulator's default is the stronger case.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// What the seeded store holds, for the tables to name rows by.
struct Fixture {
    /// Two Verified Access instances a group may be created on.
    instance_ids: Vec<String>,
    /// A subnet an endpoint on a created group may name.
    subnet_id: String,
    /// A security group an endpoint on a created group may be associated with.
    security_group_id: String,
}

/// Stand up an emulator and seed the resources a create references, the way the creating
/// operations would.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>, Fixture) {
    let (router, engine) = super::ec2_query::emulator();
    let fixture = seed(&engine);
    (router, engine, fixture)
}

/// Write the referenced resources into the engine's store and report the ids they were
/// given.
fn seed(engine: &engine::InMemoryEngine) -> Fixture {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();

    let instance_ids: Vec<String> = ["the sales applications", "the support applications"]
        .into_iter()
        .map(|description| {
            guard.insert_verified_access_instance(
                VerifiedAccessInstance {
                    description: Some(description.to_string()),
                    ..VerifiedAccessInstance::default()
                },
                None,
            )
        })
        .collect();

    let vpc_id = guard.insert_vpc("10.0.0.0/16", false, "default", "available", REGION, Vec::new());
    let zone = &availability_zones_in_region(REGION)[0];
    let subnet_id = guard.insert_subnet(SubnetSpec {
        vpc_id: vpc_id.clone(),
        cidr_block: "10.0.0.0/24".to_string(),
        availability_zone: zone.zone_name.clone(),
        availability_zone_id: zone.zone_id.clone(),
        region: REGION.to_string(),
        state: "available".to_string(),
        default_for_az: false,
        map_public_ip_on_launch: false,
        tags: Vec::new(),
    });
    let security_group_id = guard.insert_security_group(SecurityGroupSpec {
        vpc_id,
        group_name: "verified-access".to_string(),
        description: "reaches the application".to_string(),
        region: REGION.to_string(),
        tags: Vec::new(),
        ip_permissions: Vec::new(),
        ip_permissions_egress: Vec::new(),
    });

    Fixture {
        instance_ids,
        subnet_id,
        security_group_id,
    }
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "CreateVerifiedAccessGroup", params).await
}

/// The parameters of a whole create: the one required member and whatever else the case
/// carries. The ONE place a create's parameters are assembled.
fn create_params(instance_id: &str, extra: Params) -> Params {
    let mut params = vec![p("VerifiedAccessInstanceId", instance_id.to_string())];
    params.extend(extra);
    params
}

/// How many groups the store holds.
fn held_groups(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_groups
        .len()
}

/// The policy document the store kept beside the group `group_id`, which the modelled group
/// shape carries no member for.
fn stored_policy_document(engine: &engine::InMemoryEngine, group_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_groups
        .get(group_id)
        .and_then(|record| record.policy_document.clone())
}

/// A `VerifiedAccessGroupId` is `vagr-` followed by 8 or 17 lowercase hex digits, the short
/// and the long EC2 id form.
fn is_group_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("vagr-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An RFC 3339 UTC timestamp, as the service renders `CreationTime` and `LastUpdatedTime`:
/// `YYYY-MM-DDThh:mm:ssZ`.
fn is_iso8601_utc(candidate: &str) -> bool {
    candidate.len() >= 20
        && candidate.ends_with('Z')
        && candidate.as_bytes()[4] == b'-'
        && candidate.as_bytes()[7] == b'-'
        && candidate.as_bytes()[10] == b'T'
        && candidate.as_bytes()[13] == b':'
}

/// `count` tag parameters for the group this call creates, with distinct keys.
fn tag_params(count: usize) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", "verified-access-group")];
    for index in 1..=count {
        params.push(p(
            &format!("TagSpecification.1.Tag.{index}.Key"),
            format!("Key{index}"),
        ));
        params.push(p(
            &format!("TagSpecification.1.Tag.{index}.Value"),
            format!("value{index}"),
        ));
    }
    params
}

/// RULE 1 over the request shapes a create may take: each one mints a group on the instance
/// it named, reports every member it set, and gets the members the service owns derived.
#[tokio::test]
async fn every_request_shape_mints_a_group_on_the_instance_it_names() {
    // (case, which seeded instance, the optional members under test, the members the
    // response must report, the policy document the store must keep).
    type Row = (
        &'static str,
        usize,
        Params,
        Vec<(&'static str, String)>,
        Option<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "nothing but the required instance",
            0,
            Vec::new(),
            vec![("customerManagedKeyEnabled", "false".to_string())],
            None,
        ),
        (
            "a description",
            0,
            vec![p("Description", "the sales group")],
            vec![("description", "the sales group".to_string())],
            None,
        ),
        (
            "a policy document, which the group shape carries no member for",
            1,
            vec![
                p("Description", "the support group"),
                p("PolicyDocument", "permit(principal, action, resource);"),
            ],
            vec![("description", "the support group".to_string())],
            Some("permit(principal, action, resource);"),
        ),
        (
            "a customer managed key",
            1,
            vec![
                p("SseSpecification.CustomerManagedKeyEnabled", "true"),
                p(
                    "SseSpecification.KmsKeyArn",
                    "arn:aws:kms:us-west-2:000000000000:key/0a1b2c3d",
                ),
            ],
            vec![
                ("customerManagedKeyEnabled", "true".to_string()),
                (
                    "kmsKeyArn",
                    "arn:aws:kms:us-west-2:000000000000:key/0a1b2c3d".to_string(),
                ),
            ],
            None,
        ),
        (
            "two tags",
            0,
            tag_params(2),
            vec![("key", "Key1".to_string()), ("value", "value1".to_string())],
            None,
        ),
    ];

    for (case, instance, extra, expected, policy_document) in rows {
        let (router, engine, fixture) = emulator();
        let instance_id = fixture.instance_ids[instance].clone();
        let (status, body) = create(&router, &create_params(&instance_id, extra)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let group_id = element(&body, "verifiedAccessGroupId").unwrap_or_default();
        assert!(
            is_group_id(&group_id),
            "case {case:?}: expected a minted group id, got {group_id:?}"
        );
        assert_eq!(
            element(&body, "verifiedAccessInstanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the group on the instance the request named, got {:?}",
            element(&body, "verifiedAccessInstanceId")
        );
        assert_eq!(
            element(&body, "verifiedAccessGroupArn").as_deref(),
            Some(
                format!("arn:aws:ec2:{REGION}:{}:verified-access-group/{group_id}", engine::emulator_account_id())
                    .as_str()
            ),
            "case {case:?}: expected the group's ARN to name the Region, the owning account \
             and the group, got {:?}",
            element(&body, "verifiedAccessGroupArn")
        );
        assert_eq!(
            element(&body, "owner").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the owning account on the group, got {:?}",
            element(&body, "owner")
        );
        let creation_time = element(&body, "creationTime").unwrap_or_default();
        assert!(
            is_iso8601_utc(&creation_time),
            "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}"
        );
        assert_eq!(
            element(&body, "lastUpdatedTime").as_deref(),
            Some(creation_time.as_str()),
            "case {case:?}: a freshly created group was last updated when it was created, \
             got {:?}",
            element(&body, "lastUpdatedTime")
        );
        assert!(
            element(&body, "deletionTime").is_none(),
            "case {case:?}: nothing deleted this group, so it reports no deletion time, got \
             {body}"
        );
        for (member, value) in &expected {
            assert_eq!(
                element(&body, member).as_deref(),
                Some(value.as_str()),
                "case {case:?}: expected {member}={value:?} from the create, got {:?}",
                element(&body, member)
            );
        }
        // The policy document the create took is not a member of the reported group shape
        // (`GetVerifiedAccessGroupPolicy` is what reports it), so no read leaks it — and it
        // is KEPT beside the record rather than dropped.
        assert!(
            !body.contains("permit(principal"),
            "case {case:?}: the group shape carries no policy document, got {body}"
        );
        assert_eq!(
            stored_policy_document(&engine, &group_id).as_deref(),
            policy_document,
            "case {case:?}: expected the store to keep the policy document the create took, \
             got {:?}",
            stored_policy_document(&engine, &group_id)
        );
        assert_eq!(
            held_groups(&engine),
            1,
            "case {case:?}: expected exactly the one created group in the store, got {}",
            held_groups(&engine)
        );
    }
}

/// RULE 2 over the forms an unresolvable reference takes: whatever the id looks like, a group
/// is never created on an instance the account does not hold.
#[tokio::test]
async fn an_instance_the_account_does_not_hold_is_refused_and_stores_nothing() {
    // (case, the id the request names) — the long and the short EC2 id form, an id of
    // another Verified Access resource kind, and a string that is no EC2 id at all.
    let rows: Vec<(&str, &str)> = vec![
        ("an unminted long-form instance id", "vai-0123456789abcdef0"),
        ("an unminted short-form instance id", "vai-01234567"),
        ("a group id", "vagr-0123456789abcdef0"),
        ("a trust provider id", "vatp-0123456789abcdef0"),
        ("an id of no EC2 resource", "not-an-instance"),
    ];

    for (case, instance_id) in rows {
        let (router, engine, _fixture) = emulator();
        let (status, body) = create(
            &router,
            &create_params(instance_id, vec![p("Description", "a group on nothing")]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unresolvable instance, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessInstanceId.NotFound",
            "case {case:?}: expected the instance's own NotFound code, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The verified-access-instance ID '{instance_id}' does not exist"),
            "case {case:?}: expected the refusal to name the id the request gave, got {}",
            error_message(&body)
        );
        assert_eq!(
            held_groups(&engine),
            0,
            "case {case:?}: a refused create stores nothing, got {} group(s)",
            held_groups(&engine)
        );
    }
}

/// RULE 3: the group is the only resource this call creates, so only its own resource type
/// may be tagged, a tag payload is validated the way every create's is, and the published
/// tag-count and client-token bounds hold — on each bound and one past it.
#[tokio::test]
async fn tag_specifications_and_the_published_bounds_are_enforced() {
    // (case, the parameters under test, the HTTP status, the error code an accepted create
    // reports as the empty string).
    let client_token = |chars: usize| "t".repeat(chars);
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        ("the group's own resource type", tag_params(1), 200, ""),
        (
            "no resource type at all",
            vec![
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "MissingParameter",
        ),
        (
            "the endpoint's resource type",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-endpoint"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "the instance's resource type",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-instance"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a VPC's resource type",
            vec![
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "one tag key twice",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-group"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
                p("TagSpecification.1.Tag.2.Key", "Team"),
                p("TagSpecification.1.Tag.2.Value", "network"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag key in the reserved namespace",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-group"),
                p("TagSpecification.1.Tag.1.Key", "aws:cloudformation:stack-name"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "as many tags as a resource carries",
            tag_params(MAX_TAGS_PER_RESOURCE),
            200,
            "",
        ),
        (
            "one tag too many",
            tag_params(MAX_TAGS_PER_RESOURCE + 1),
            400,
            "TagLimitExceeded",
        ),
        (
            "as long a client token as the service takes",
            vec![p("ClientToken", client_token(MAX_CLIENT_TOKEN_CHARS))],
            200,
            "",
        ),
        (
            "one client token character too many",
            vec![p("ClientToken", client_token(MAX_CLIENT_TOKEN_CHARS + 1))],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (router, engine, fixture) = emulator();
        let (status, body) = create(
            &router,
            &create_params(&fixture.instance_ids[0], params),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                held_groups(&engine),
                1,
                "case {case:?}: an accepted create stores its group, got {} group(s)",
                held_groups(&engine)
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the {expected_code} refusal, got {body}"
            );
            assert_eq!(
                held_groups(&engine),
                0,
                "case {case:?}: a refused create stores nothing, got {} group(s)",
                held_groups(&engine)
            );
        }
    }
}

/// RULE 3's store ceiling: the store holds at most [`MAX_VERIFIED_ACCESS_GROUPS`] groups, so
/// one more is refused with the quota code rather than growing the store without bound. The
/// groups already held are seeded through the insert seam, the same writer the create uses.
#[tokio::test]
async fn one_group_past_the_store_ceiling_is_refused() {
    let (router, engine, fixture) = emulator();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        while guard.verified_access_groups.len() < MAX_VERIFIED_ACCESS_GROUPS {
            guard.insert_verified_access_group(
                VerifiedAccessGroupRecord {
                    view: VerifiedAccessGroup {
                        verified_access_instance_id: Some(fixture.instance_ids[0].clone()),
                        ..VerifiedAccessGroup::default()
                    },
                    ..VerifiedAccessGroupRecord::default()
                },
                REGION,
                None,
            );
        }
    }

    let (status, body) = create(
        &router,
        &create_params(&fixture.instance_ids[0], vec![p("Description", "one too many")]),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 once the store is full, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "VerifiedAccessGroupLimitExceeded",
        "expected the group's own quota code, got {body}"
    );
    assert_eq!(
        held_groups(&engine),
        MAX_VERIFIED_ACCESS_GROUPS,
        "a refused create stores nothing, got {} group(s)",
        held_groups(&engine)
    );
}

/// RULE 4: one `ClientToken` names the one group it minted, whatever the rest of the repeat
/// says, and a token the store has not seen mints a group of its own.
#[tokio::test]
async fn one_client_token_names_one_group() {
    let (router, engine, fixture) = emulator();
    let instance_id = fixture.instance_ids[0].clone();

    let (status, body) = create(
        &router,
        &create_params(
            &instance_id,
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "the group under test"),
            ],
        ),
    )
    .await;
    assert_eq!(status, 200, "the first create must succeed, got {status}: {body}");
    let first = element(&body, "verifiedAccessGroupId").unwrap_or_default();

    // (case, the parameters of the repeat, the id it must report, how many groups the store
    // must hold afterwards).
    let rows: Vec<(&str, Params, String, usize)> = vec![
        (
            "the same token and the same request",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "the group under test"),
            ],
            first.clone(),
            1,
        ),
        (
            "the same token and a different description",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "a different description"),
            ],
            first.clone(),
            1,
        ),
        (
            "a token the store has not seen",
            vec![
                p("ClientToken", "the-second-token"),
                p("Description", "the second group"),
            ],
            String::new(),
            2,
        ),
    ];

    for (case, params, expected_id, expected_held) in rows {
        let (status, body) = create(&router, &create_params(&instance_id, params)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let group_id = element(&body, "verifiedAccessGroupId").unwrap_or_default();
        if expected_id.is_empty() {
            assert!(
                is_group_id(&group_id) && group_id != first,
                "case {case:?}: expected a group of its own, got {group_id:?}"
            );
        } else {
            assert_eq!(
                group_id, expected_id,
                "case {case:?}: expected the group the token already minted, got {group_id:?}"
            );
            assert_eq!(
                element(&body, "description").as_deref(),
                Some("the group under test"),
                "case {case:?}: a replay reports the group it minted, not the repeat's own \
                 members, got {:?}",
                element(&body, "description")
            );
        }
        assert_eq!(
            held_groups(&engine),
            expected_held,
            "case {case:?}: expected {expected_held} group(s) in the store, got {}",
            held_groups(&engine)
        );
    }
}

/// RULE 4's `DryRun`: the flag reports the outcome the request WOULD have had — the refusal
/// when it would have been refused — and writes nothing either way.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    // (case, the instance the request names, the HTTP status, the error code).
    let rows: Vec<(&str, Option<usize>, u16, &str)> = vec![
        (
            "a create that would have succeeded",
            Some(0),
            412,
            "DryRunOperation",
        ),
        (
            "a create that would have been refused",
            None,
            400,
            "InvalidVerifiedAccessInstanceId.NotFound",
        ),
    ];

    for (case, instance, expected_status, expected_code) in rows {
        let (router, engine, fixture) = emulator();
        let instance_id = match instance {
            Some(index) => fixture.instance_ids[index].clone(),
            None => "vai-0123456789abcdef0".to_string(),
        };
        let (status, body) = create(
            &router,
            &create_params(
                &instance_id,
                vec![p("DryRun", "true"), p("Description", "a dry run")],
            ),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected the {expected_code} refusal, got {body}"
        );
        assert_eq!(
            held_groups(&engine),
            0,
            "case {case:?}: a dry run writes nothing, got {} group(s)",
            held_groups(&engine)
        );
    }
}

/// RULE 5: the created group is in the STORE, so the operations that resolve a group reach
/// it — `CreateVerifiedAccessEndpoint` attaches an endpoint to it and reports the instance
/// read off the GROUP, and `DescribeVerifiedAccessEndpoints` narrows by it — while a group id
/// this account never minted is refused by the same reader.
#[tokio::test]
async fn the_created_group_is_reachable_through_the_operations_that_resolve_one() {
    for index in 0..2 {
        let (router, _engine, fixture) = emulator();
        let instance_id = fixture.instance_ids[index].clone();
        let case = format!("a group on instance {index}");

        let (status, body) = create(
            &router,
            &create_params(&instance_id, vec![p("Description", "the group under test")]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the create must succeed, got {status} with body {body}"
        );
        let group_id = element(&body, "verifiedAccessGroupId").unwrap_or_default();

        // An endpoint on the created group: the instance it reports comes from the stored
        // GROUP, not from the endpoint request, which names no instance at all.
        let endpoint_params: Params = [
            vec![
                p("VerifiedAccessGroupId", group_id.clone()),
                p("EndpointType", "cidr"),
                p("AttachmentType", "vpc"),
                p("CidrOptions.Protocol", "tcp"),
                p("CidrOptions.Cidr", "10.0.0.0/24"),
                p("CidrOptions.PortRange.1.FromPort", "5000"),
                p("CidrOptions.PortRange.1.ToPort", "5000"),
            ],
            indexed("CidrOptions.SubnetId", &[fixture.subnet_id.as_str()]),
            indexed("SecurityGroupId", &[fixture.security_group_id.as_str()]),
        ]
        .concat();
        let (status, endpoint_body) =
            call(&router, REGION, "CreateVerifiedAccessEndpoint", &endpoint_params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: an endpoint on the created group must be accepted, got {status} \
             with body {endpoint_body}"
        );
        let endpoint_id = element(&endpoint_body, "verifiedAccessEndpointId").unwrap_or_default();
        assert_eq!(
            element(&endpoint_body, "verifiedAccessInstanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the endpoint to report the instance of its group, got {:?}",
            element(&endpoint_body, "verifiedAccessInstanceId")
        );

        // The describe narrows by the created group, and refuses a group id nothing minted.
        let (status, described) = call(
            &router,
            REGION,
            "DescribeVerifiedAccessEndpoints",
            &[p("VerifiedAccessGroupId", group_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the describe to accept the created group, got {status} \
             with body {described}"
        );
        assert_eq!(
            elements(&described, "verifiedAccessEndpointId"),
            vec![endpoint_id.clone()],
            "case {case:?}: expected the endpoint of the created group, got {described}"
        );

        let (status, refused) = call(
            &router,
            REGION,
            "DescribeVerifiedAccessEndpoints",
            &[p("VerifiedAccessGroupId", "vagr-0123456789abcdef0")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a group id nothing minted to be refused, got {status} \
             with body {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidVerifiedAccessGroupId.NotFound",
            "case {case:?}: expected the group's own NotFound code, got {refused}"
        );
    }
}
