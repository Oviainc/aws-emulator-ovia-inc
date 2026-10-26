// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access INSTANCE slice: `CreateVerifiedAccessInstance`,
//! driven over the wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateVerifiedAccessInstance` MINTS A REGIONAL INSTANCE FROM THE REQUEST: every member
//!    the request sets round-trips through the response and the store, the members the
//!    service owns (the instance id and the timestamps) are derived rather than taken, and a
//!    fresh instance carries no trust provider because a create attaches none;
//! 2. the instance is in the STORE rather than echoed: `CreateVerifiedAccessGroup` resolves
//!    the instance this call created and reports it on the group, while an instance id
//!    nothing minted is refused by that same reader;
//! 3. the instance is the only resource this call creates, so a tag specification naming
//!    another resource type is refused, the published tag-count and client-token bounds hold,
//!    and a custom subdomain that names no domain is refused;
//! 4. one `ClientToken` names one instance, and a `DryRun` reports the outcome the request
//!    would have had and WRITES NOTHING;
//! 5. the store holds at most [`MAX_VERIFIED_ACCESS_INSTANCES`] instances, so one more is
//!    refused with the instance's own quota code.
//!
//! Each table row is a distinct input along what the rule generalises over — which optional
//! members the request carries, whether FIPS support is asked for and in which form, which
//! resource type a tag specification names, how many tags, how long a client token is, which
//! reference is unresolvable and in which form — so no row is the specification.

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_CLIENT_TOKEN_CHARS, MAX_TAGS_PER_RESOURCE,
    MAX_VERIFIED_ACCESS_INSTANCES,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message};

/// The Region every call here is signed for: a Region that is not the emulator's default is
/// the stronger case, and the instance is a regional entity.
const REGION: &str = "eu-west-1";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// Stand up an emulator with nothing seeded: the account starts with no Verified Access
/// instance, so every instance a case reads back is one a create under test minted.
fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn create(router: &MultiServiceRouter, params: &Params) -> (u16, String) {
    call(router, REGION, "CreateVerifiedAccessInstance", params).await
}

/// How many Verified Access instances the store holds.
fn held_instances(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instances
        .len()
}

/// The description the store kept for the instance `instance_id`, or `None` when the store
/// holds no such instance.
fn stored_description(engine: &engine::InMemoryEngine, instance_id: &str) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .verified_access_instances
        .get(instance_id)
        .and_then(|instance| instance.description.clone())
}

/// A `VerifiedAccessInstanceId` is `vai-` followed by 8 or 17 lowercase hex digits, the short
/// and the long EC2 id form.
fn is_instance_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("vai-") else {
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

/// `count` tag parameters for the instance this call creates, with distinct keys.
fn tag_params(count: usize) -> Params {
    let mut params = vec![p("TagSpecification.1.ResourceType", "verified-access-instance")];
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

/// RULE 1 over the request shapes a create may take: each one mints an instance reporting
/// every member it set, and gets the members the service owns derived.
#[tokio::test]
async fn every_request_shape_mints_an_instance_reporting_what_it_set() {
    // (case, the request's optional members, the members the response must report, the
    // members it must NOT report at all).
    type Row = (
        &'static str,
        Params,
        Vec<(&'static str, &'static str)>,
        Vec<&'static str>,
    );
    let rows: Vec<Row> = vec![
        (
            "nothing optional at all",
            Vec::new(),
            // FIPS support is reported either way, and a request that asked for neither
            // answer gets it off.
            vec![("fipsEnabled", "false")],
            vec!["description", "subDomain"],
        ),
        (
            "a description",
            vec![p("Description", "the sales applications")],
            vec![
                ("description", "the sales applications"),
                ("fipsEnabled", "false"),
            ],
            vec!["subDomain"],
        ),
        (
            "FIPS support asked for",
            vec![p("FIPSEnabled", "true")],
            vec![("fipsEnabled", "true")],
            vec!["description", "subDomain"],
        ),
        (
            "FIPS support declined explicitly",
            vec![p("FIPSEnabled", "false")],
            vec![("fipsEnabled", "false")],
            vec!["description", "subDomain"],
        ),
        (
            "a custom subdomain for the network CIDR endpoints",
            vec![p("CidrEndpointsCustomSubDomain", "access.example.com")],
            vec![
                ("subDomain", "access.example.com"),
                ("fipsEnabled", "false"),
            ],
            vec!["description"],
        ),
        (
            "every optional member at once",
            vec![
                p("Description", "the support applications"),
                p("FIPSEnabled", "true"),
                p("CidrEndpointsCustomSubDomain", "support.example.org"),
            ],
            vec![
                ("description", "the support applications"),
                ("fipsEnabled", "true"),
                ("subDomain", "support.example.org"),
            ],
            Vec::new(),
        ),
        (
            "two tags",
            tag_params(2),
            vec![("key", "Key1"), ("value", "value1")],
            vec!["description", "subDomain"],
        ),
    ];

    for (case, extra, expected, absent) in rows {
        let (router, engine) = emulator();
        let (status, body) = create(&router, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        let instance_id = element(&body, "verifiedAccessInstanceId").unwrap_or_default();
        assert!(
            is_instance_id(&instance_id),
            "case {case:?}: expected a minted instance id, got {instance_id:?}"
        );
        let creation_time = element(&body, "creationTime").unwrap_or_default();
        assert!(
            is_iso8601_utc(&creation_time),
            "case {case:?}: expected an ISO 8601 creationTime, got {creation_time:?}"
        );
        assert_eq!(
            element(&body, "lastUpdatedTime").as_deref(),
            Some(creation_time.as_str()),
            "case {case:?}: a freshly created instance was last updated when it was created, \
             got {:?}",
            element(&body, "lastUpdatedTime")
        );
        // A create attaches no trust provider, so the reported set carries no id.
        assert!(
            elements(&body, "verifiedAccessTrustProviderId").is_empty(),
            "case {case:?}: a fresh instance has no trust provider attached, got {body}"
        );
        for (member, value) in &expected {
            assert_eq!(
                element(&body, member).as_deref(),
                Some(*value),
                "case {case:?}: expected {member}={value:?} from the create, got {:?}",
                element(&body, member)
            );
        }
        for member in &absent {
            assert!(
                element(&body, member).is_none(),
                "case {case:?}: the request set no {member}, so the instance reports none, \
                 got {body}"
            );
        }

        // The instance is in the store under the id the response reported, with the
        // description the request gave it.
        assert_eq!(
            held_instances(&engine),
            1,
            "case {case:?}: expected exactly the one created instance in the store, got {}",
            held_instances(&engine)
        );
        assert_eq!(
            stored_description(&engine, &instance_id).as_deref(),
            expected
                .iter()
                .find(|(member, _)| *member == "description")
                .map(|(_, value)| *value),
            "case {case:?}: expected the store to keep the description the create took, got \
             {:?}",
            stored_description(&engine, &instance_id)
        );
    }
}

/// RULE 2: the created instance is in the STORE, so the operation that resolves an instance
/// reaches it — `CreateVerifiedAccessGroup` mints a group on it and reports its id — while an
/// instance id this account never minted is refused by that same reader, whatever the id
/// looks like.
#[tokio::test]
async fn the_created_instance_is_reachable_through_the_operation_that_resolves_one() {
    // Two creates in one emulator: each group must land on the instance it named, so a
    // reader that resolved any instance at all would not pass.
    let (router, engine) = emulator();
    let mut created: Vec<String> = Vec::new();
    for description in ["the sales applications", "the support applications"] {
        let (status, body) = create(&router, &vec![p("Description", description)]).await;
        assert_eq!(
            status, 200,
            "case {description:?}: the create must succeed, got {status} with body {body}"
        );
        created.push(element(&body, "verifiedAccessInstanceId").unwrap_or_default());
    }
    assert_eq!(
        held_instances(&engine),
        2,
        "two creates store two instances, got {}",
        held_instances(&engine)
    );

    for instance_id in &created {
        let (status, body) = call(
            &router,
            REGION,
            "CreateVerifiedAccessGroup",
            &[p("VerifiedAccessInstanceId", instance_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {instance_id:?}: a group on the created instance must be accepted, got \
             {status} with body {body}"
        );
        assert_eq!(
            element(&body, "verifiedAccessInstanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {instance_id:?}: expected the group on the instance the request named, got \
             {:?}",
            element(&body, "verifiedAccessInstanceId")
        );
    }

    // (case, an id no create minted) — the long and the short EC2 id form, an id of another
    // Verified Access resource kind, and a string that is no EC2 id at all.
    let unminted: Vec<(&str, &str)> = vec![
        ("an unminted long-form instance id", "vai-0123456789abcdef0"),
        ("an unminted short-form instance id", "vai-01234567"),
        ("a group id", "vagr-0123456789abcdef0"),
        ("an id of no EC2 resource", "not-an-instance"),
    ];
    for (case, instance_id) in unminted {
        let (status, body) = call(
            &router,
            REGION,
            "CreateVerifiedAccessGroup",
            &[p("VerifiedAccessInstanceId", instance_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an instance nothing minted, got {status} \
             with body {body}"
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
    }
}

/// RULE 3: the instance is the only resource this call creates, so only its own resource type
/// may be tagged, a tag payload is validated the way every create's is, the published
/// tag-count and client-token bounds hold — on each bound and one past it — and a custom
/// subdomain that names no domain is refused.
#[tokio::test]
async fn tag_specifications_and_the_published_bounds_are_enforced() {
    // (case, the parameters under test, the HTTP status, the error code an accepted create
    // reports as the empty string).
    let client_token = |chars: usize| "t".repeat(chars);
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        ("the instance's own resource type", tag_params(1), 200, ""),
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
            "the group's resource type",
            vec![
                p("TagSpecification.1.ResourceType", "verified-access-group"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "the trust provider's resource type",
            vec![
                p(
                    "TagSpecification.1.ResourceType",
                    "verified-access-trust-provider",
                ),
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
                p("TagSpecification.1.ResourceType", "verified-access-instance"),
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
                p("TagSpecification.1.ResourceType", "verified-access-instance"),
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
        (
            "a subdomain that is the empty name",
            vec![p("CidrEndpointsCustomSubDomain", "")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a subdomain that is nothing but blanks",
            vec![p("CidrEndpointsCustomSubDomain", "   ")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 200 {
            assert_eq!(
                held_instances(&engine),
                1,
                "case {case:?}: an accepted create stores its instance, got {} instance(s)",
                held_instances(&engine)
            );
        } else {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected the {expected_code} refusal, got {body}"
            );
            assert_eq!(
                held_instances(&engine),
                0,
                "case {case:?}: a refused create stores nothing, got {} instance(s)",
                held_instances(&engine)
            );
        }
    }
}

/// RULE 4: one `ClientToken` names the one instance it minted, whatever the rest of the
/// repeat says, and a token the store has not seen mints an instance of its own.
#[tokio::test]
async fn one_client_token_names_one_instance() {
    let (router, engine) = emulator();
    let (status, body) = create(
        &router,
        &vec![
            p("ClientToken", "the-first-token"),
            p("Description", "the instance under test"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the first create must succeed, got {status}: {body}"
    );
    let first = element(&body, "verifiedAccessInstanceId").unwrap_or_default();

    // (case, the parameters of the repeat, the id it must report, how many instances the
    // store must hold afterwards).
    let rows: Vec<(&str, Params, String, usize)> = vec![
        (
            "the same token and the same request",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "the instance under test"),
            ],
            first.clone(),
            1,
        ),
        (
            "the same token and a different description",
            vec![
                p("ClientToken", "the-first-token"),
                p("Description", "a different description"),
                p("FIPSEnabled", "true"),
            ],
            first.clone(),
            1,
        ),
        (
            "a token the store has not seen",
            vec![
                p("ClientToken", "the-second-token"),
                p("Description", "the second instance"),
            ],
            String::new(),
            2,
        ),
    ];

    for (case, params, expected_id, expected_held) in rows {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let instance_id = element(&body, "verifiedAccessInstanceId").unwrap_or_default();
        if expected_id.is_empty() {
            assert!(
                is_instance_id(&instance_id) && instance_id != first,
                "case {case:?}: expected an instance of its own, got {instance_id:?}"
            );
        } else {
            assert_eq!(
                instance_id, expected_id,
                "case {case:?}: expected the instance the token already minted, got \
                 {instance_id:?}"
            );
            assert_eq!(
                element(&body, "description").as_deref(),
                Some("the instance under test"),
                "case {case:?}: a replay reports the instance it minted, not the repeat's own \
                 members, got {:?}",
                element(&body, "description")
            );
            assert_eq!(
                element(&body, "fipsEnabled").as_deref(),
                Some("false"),
                "case {case:?}: a replay reports the instance it minted, not the repeat's own \
                 members, got {:?}",
                element(&body, "fipsEnabled")
            );
        }
        assert_eq!(
            held_instances(&engine),
            expected_held,
            "case {case:?}: expected {expected_held} instance(s) in the store, got {}",
            held_instances(&engine)
        );
    }
}

/// RULE 4's `DryRun`: the flag reports the outcome the request WOULD have had — the refusal
/// when it would have been refused — and writes nothing either way.
#[tokio::test]
async fn a_dry_run_reports_the_outcome_and_writes_nothing() {
    // (case, the parameters beside the flag, the HTTP status, the error code).
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a create that would have succeeded",
            vec![p("Description", "a dry run")],
            412,
            "DryRunOperation",
        ),
        (
            "a create that would have been refused for its tag specification",
            vec![
                p("TagSpecification.1.ResourceType", "vpc"),
                p("TagSpecification.1.Tag.1.Key", "Team"),
                p("TagSpecification.1.Tag.1.Value", "platform"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a create that would have been refused for its subdomain",
            vec![p("CidrEndpointsCustomSubDomain", "")],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, extra, expected_status, expected_code) in rows {
        let (router, engine) = emulator();
        let mut params = vec![p("DryRun", "true")];
        params.extend(extra);
        let (status, body) = create(&router, &params).await;
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
            held_instances(&engine),
            0,
            "case {case:?}: a dry run writes nothing, got {} instance(s)",
            held_instances(&engine)
        );
    }
}

/// RULE 5: the store holds at most [`MAX_VERIFIED_ACCESS_INSTANCES`] instances, so one more
/// is refused with the instance's own quota code rather than growing the store without
/// bound. The instances already held are seeded through the insert seam, the same writer the
/// create uses.
#[tokio::test]
async fn one_instance_past_the_store_ceiling_is_refused() {
    let (router, engine) = emulator();
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        while guard.verified_access_instances.len() < MAX_VERIFIED_ACCESS_INSTANCES {
            guard.insert_verified_access_instance(
                ec2::ops::types::VerifiedAccessInstance::default(),
                None,
            );
        }
    }

    let (status, body) = create(&router, &vec![p("Description", "one too many")]).await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 once the store is full, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "VerifiedAccessInstanceLimitExceeded",
        "expected the instance's own quota code, got {body}"
    );
    assert_eq!(
        held_instances(&engine),
        MAX_VERIFIED_ACCESS_INSTANCES,
        "a refused create stores nothing beyond the ceiling, got {} instance(s)",
        held_instances(&engine)
    );
}
