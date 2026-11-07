// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Verified Access INSTANCE slice: `CreateVerifiedAccessInstance`
//! and `DescribeVerifiedAccessInstances`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
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
//!    refused with the instance's own quota code;
//! 6. `DescribeVerifiedAccessInstances` REPORTS THE STORE: a created instance reads back with
//!    every member it was created with, the report is narrowed by `VerifiedAccessInstanceIds`
//!    and by every filter the operation answers (AND across filters, OR within one, `*`
//!    wildcards), an id the account holds no instance for is refused, and the collection is
//!    paged by `MaxResults`/`NextToken`.
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

use super::ec2_query::{
    call, element, elements, error_code, error_message, filters, indexed, unescaped,
};

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

/// Ask `DescribeVerifiedAccessInstances` with `params`.
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeVerifiedAccessInstances", params).await
}

/// Create one instance that must succeed, and answer the id it was minted.
async fn seed(router: &MultiServiceRouter, case: &str, params: &Params) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVerifiedAccessInstance, got {status} with \
         body {body}"
    );
    let instance_id = element(&body, "verifiedAccessInstanceId").unwrap_or_default();
    assert!(
        is_instance_id(&instance_id),
        "case {case:?}: expected a minted instance id, got {instance_id:?}"
    );
    instance_id
}

/// The ids `DescribeVerifiedAccessInstances` reports for `params`, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVerifiedAccessInstances, got {status} \
         with body {body}"
    );
    elements(&body, "verifiedAccessInstanceId")
}

/// RULE 6: every shape a create may take reads back out of the STORE through
/// `DescribeVerifiedAccessInstances` carrying exactly the members it was created with, with
/// the members the service owns derived — so the describe reports the stored instance rather
/// than an echo of the describe's own request.
#[tokio::test]
async fn every_created_instance_reads_back_through_the_describe() {
    // (case, the create's optional members, the members the describe must report, the members
    // it must NOT report at all).
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
            "a custom subdomain for the network CIDR endpoints",
            vec![p("CidrEndpointsCustomSubDomain", "access.example.com")],
            vec![
                ("subDomain", "access.example.com"),
                ("fipsEnabled", "false"),
            ],
            vec!["description"],
        ),
        (
            "every optional member at once, with tags",
            [
                vec![
                    p("Description", "the support applications"),
                    p("FIPSEnabled", "true"),
                    p("CidrEndpointsCustomSubDomain", "support.example.org"),
                ],
                tag_params(2),
            ]
            .concat(),
            vec![
                ("description", "the support applications"),
                ("fipsEnabled", "true"),
                ("subDomain", "support.example.org"),
                ("key", "Key1"),
                ("value", "value1"),
            ],
            Vec::new(),
        ),
    ];

    for (case, create_params, expected, absent) in rows {
        let (router, engine) = emulator();
        let instance_id = seed(&router, case, &create_params).await;

        let (status, body) = describe(
            &router,
            &indexed("VerifiedAccessInstanceId", &[instance_id.as_str()]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the describe, got {status} with body {body}"
        );
        assert_eq!(
            elements(&body, "verifiedAccessInstanceId"),
            vec![instance_id.clone()],
            "case {case:?}: expected exactly the created instance, got {body}"
        );
        let creation_time = element(&body, "creationTime").unwrap_or_default();
        assert!(
            is_iso8601_utc(&creation_time),
            "case {case:?}: expected an ISO 8601 creationTime from the describe, got \
             {creation_time:?}"
        );
        assert_eq!(
            element(&body, "lastUpdatedTime").as_deref(),
            Some(creation_time.as_str()),
            "case {case:?}: nothing updated the instance since it was created, got {:?}",
            element(&body, "lastUpdatedTime")
        );
        assert!(
            elements(&body, "verifiedAccessTrustProviderId").is_empty(),
            "case {case:?}: a create attaches no trust provider, got {body}"
        );
        for (member, value) in &expected {
            assert_eq!(
                element(&body, member).as_deref(),
                Some(*value),
                "case {case:?}: expected {member}={value:?} from the describe, got {:?}",
                element(&body, member)
            );
        }
        for member in &absent {
            assert!(
                element(&body, member).is_none(),
                "case {case:?}: the create set no {member}, so the describe reports none, got \
                 {body}"
            );
        }

        // A read changes nothing a later caller sees: the store still holds the one instance.
        assert_eq!(
            held_instances(&engine),
            1,
            "case {case:?}: a describe writes nothing, got {} instance(s)",
            held_instances(&engine)
        );
    }
}

/// RULE 6: the describe reports the store narrowed by `VerifiedAccessInstanceIds` and by
/// every filter it answers, with EC2's AND across filters, OR within one filter, and `*`
/// wildcards; an id the account holds no instance for is refused, a filter name the operation
/// does not answer is refused, and a `DryRun` reports the outcome and changes nothing.
#[tokio::test]
async fn the_describe_narrows_by_id_and_by_every_answered_filter() {
    let (router, _engine) = emulator();

    // Instances differing along every attribute the filters select on.
    struct Seed {
        description: &'static str,
        fips: &'static str,
        sub_domain: Option<&'static str>,
        team: &'static str,
    }
    let seeds = [
        Seed {
            description: "sales-apps",
            fips: "true",
            sub_domain: Some("sales.example.com"),
            team: "platform",
        },
        Seed {
            description: "staff-apps",
            fips: "false",
            sub_domain: Some("staff.example.com"),
            team: "platform",
        },
        Seed {
            description: "sales-tools",
            fips: "false",
            sub_domain: None,
            team: "endpoint",
        },
        Seed {
            description: "field-tools",
            fips: "true",
            sub_domain: None,
            team: "endpoint",
        },
    ];

    let mut created: Vec<String> = Vec::new();
    for entry in &seeds {
        let mut params = vec![
            p("Description", entry.description),
            p("FIPSEnabled", entry.fips),
            p("TagSpecification.1.ResourceType", "verified-access-instance"),
            p("TagSpecification.1.Tag.1.Key", "Team"),
            p("TagSpecification.1.Tag.1.Value", entry.team),
        ];
        if let Some(sub_domain) = entry.sub_domain {
            params.push(p("CidrEndpointsCustomSubDomain", sub_domain));
        }
        created.push(seed(&router, entry.description, &params).await);
    }

    // The describe reports and pages in id order.
    let mut in_id_order = created.clone();
    in_id_order.sort();
    assert_eq!(
        described_ids(&router, "every stored instance", &[]).await,
        in_id_order,
        "a describe with no narrowing reports every stored instance, in id order"
    );

    let only = |indexes: &[usize]| -> Vec<String> {
        let mut ids: Vec<String> = indexes.iter().map(|index| created[*index].clone()).collect();
        ids.sort();
        ids
    };

    // (case, the narrowing parameters, the ids it must select).
    let rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "VerifiedAccessInstanceIds names two of the four".to_string(),
            indexed(
                "VerifiedAccessInstanceId",
                &[created[0].as_str(), created[3].as_str()],
            ),
            only(&[0, 3]),
        ),
        (
            "description, exactly".to_string(),
            filters(&[("description", &["staff-apps"])]),
            only(&[1]),
        ),
        (
            "description, with a wildcard".to_string(),
            filters(&[("description", &["sales-*"])]),
            only(&[0, 2]),
        ),
        (
            "two values of one filter are joined with OR".to_string(),
            filters(&[("description", &["staff-apps", "field-tools"])]),
            only(&[1, 3]),
        ),
        (
            "fips-enabled=true".to_string(),
            filters(&[("fips-enabled", &["true"])]),
            only(&[0, 3]),
        ),
        (
            "fips-enabled=false".to_string(),
            filters(&[("fips-enabled", &["false"])]),
            only(&[1, 2]),
        ),
        (
            "cidr-endpoints-custom-sub-domain, exactly".to_string(),
            filters(&[("cidr-endpoints-custom-sub-domain", &["staff.example.com"])]),
            only(&[1]),
        ),
        (
            "cidr-endpoints-custom-sub-domain, with a wildcard".to_string(),
            filters(&[("cidr-endpoints-custom-sub-domain", &["*.example.com"])]),
            only(&[0, 1]),
        ),
        (
            "verified-access-instance-id".to_string(),
            filters(&[("verified-access-instance-id", &[created[2].as_str()])]),
            only(&[2]),
        ),
        (
            "tag:Team".to_string(),
            filters(&[("tag:Team", &["endpoint"])]),
            only(&[2, 3]),
        ),
        (
            "tag-key".to_string(),
            filters(&[("tag-key", &["Team"])]),
            only(&[0, 1, 2, 3]),
        ),
        (
            "three filters are joined with AND".to_string(),
            filters(&[
                ("tag:Team", &["endpoint"]),
                ("fips-enabled", &["false"]),
                ("description", &["sales-*"]),
            ]),
            only(&[2]),
        ),
        (
            "a filter AND an id narrowing".to_string(),
            [
                indexed(
                    "VerifiedAccessInstanceId",
                    &[created[0].as_str(), created[1].as_str()],
                ),
                filters(&[("fips-enabled", &["false"])]),
            ]
            .concat(),
            only(&[1]),
        ),
        (
            "a value no instance carries".to_string(),
            filters(&[("description", &["no-such-apps"])]),
            Vec::new(),
        ),
        (
            "an attribute the instance does not carry".to_string(),
            filters(&[("cidr-endpoints-custom-sub-domain", &["sales-tools"])]),
            Vec::new(),
        ),
        (
            "a filter with no value at all matches nothing".to_string(),
            filters(&[("description", &[])]),
            Vec::new(),
        ),
        (
            "a trust provider id, which no create attaches".to_string(),
            filters(&[(
                "verified-access-trust-provider-id",
                &["vatp-0123456789abcdef0"],
            )]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in rows {
        assert_eq!(
            described_ids(&router, &case, &params).await,
            expected,
            "case {case:?}: expected exactly those instances"
        );
    }

    // `creation-time` and `last-updated-time` are the two answered filters whose values the
    // SERVICE derives (from its own clock), so their rows are computed from what the describe
    // reports rather than written out: filtering on a timestamp selects exactly the instances
    // carrying it, and a wildcard over the date part selects every one created on that date.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"clock filters\": expected HTTP 200, got {status} with body {body}"
    );
    let reported = elements(&body, "verifiedAccessInstanceId");
    for (name, member) in [
        ("creation-time", "creationTime"),
        ("last-updated-time", "lastUpdatedTime"),
    ] {
        let times = elements(&body, member);
        assert_eq!(
            times.len(),
            reported.len(),
            "case {name:?}: expected one {member} per reported instance, got {times:?}"
        );
        let probe = times.first().cloned().unwrap_or_default();
        let mut carrying_probe: Vec<String> = reported
            .iter()
            .zip(&times)
            .filter(|(_, time)| *time == &probe)
            .map(|(id, _)| id.clone())
            .collect();
        carrying_probe.sort();
        let exact = described_ids(
            &router,
            &format!("{name}={probe}"),
            &filters(&[(name, &[probe.as_str()])]),
        )
        .await;
        assert_eq!(
            exact, carrying_probe,
            "case {name:?}={probe:?}: expected exactly the instances carrying that timestamp, \
             got {exact:?}"
        );
        // The date part of an RFC 3339 timestamp is its first 10 characters; every instance
        // here was created by the same call sequence, so all of them carry it.
        let date = probe.get(..10).unwrap_or_default();
        let pattern = format!("{date}*");
        let wildcarded = described_ids(
            &router,
            &format!("{name}={pattern}"),
            &filters(&[(name, &[pattern.as_str()])]),
        )
        .await;
        assert_eq!(
            wildcarded, in_id_order,
            "case {name:?}={pattern:?}: expected every instance created on that date, got \
             {wildcarded:?}"
        );
    }

    // An id that names no instance of the account is an error, not an absent row.
    for instance_id in [
        "vai-00000000000000999",
        "vai-0000ffff",
        "not-an-id",
        &format!("{}x", created[0]),
    ] {
        let (status, body) =
            describe(&router, &indexed("VerifiedAccessInstanceId", &[instance_id])).await;
        assert_eq!(
            status, 400,
            "case {instance_id:?}: expected HTTP 400 for an unknown id, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVerifiedAccessInstanceId.NotFound",
            "case {instance_id:?}: expected the instance's own NotFound code, got {body}"
        );
        // The message is built from the request, so the caller can tell WHICH of the ids it
        // named is unknown. The sentence is this operation's own — a report of the instances
        // words the refusal differently from a mutation of one, which answers `The
        // verified-access-instance ID '<id>' does not exist`.
        assert_eq!(
            unescaped(&error_message(&body)),
            format!("VerifiedAccessInstance {instance_id} does not exist"),
            "case {instance_id:?}: expected the refusal to name the id the request gave, got {}",
            error_message(&body)
        );
    }

    // A filter name this operation does not answer is refused, rather than read as "no
    // results".
    for name in [
        "fipsEnabled",
        "tag:",
        "verified-access-instance",
        "FIPS-ENABLED",
        "verified-access-group-id",
    ] {
        let (status, body) = describe(&router, &filters(&[(name, &["true"])])).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for an unanswered filter, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // DryRun reports what the request WOULD have done and changes nothing.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"DryRun\": expected DryRunOperation, got {body}"
    );
    assert_eq!(
        described_ids(&router, "after a DryRun", &[]).await,
        in_id_order,
        "case \"DryRun\": a dry run changes nothing a later caller sees"
    );
}

/// RULE 6's trust provider filter: `verified-access-trust-provider-id` selects on the ids of
/// the trust providers ATTACHED to an instance
/// (`VerifiedAccessInstance$VerifiedAccessTrustProviders`), so it selects the instance
/// carrying that id and no other.
///
/// The attachments are seeded through the store's own insert seam — the writer the create uses
/// — so this case drives the filter over trust provider ids no create of a trust provider has
/// to mint first; what `AttachVerifiedAccessTrustProvider` itself writes is covered by
/// [`super::verified_access_trust_provider_attachment`].
#[tokio::test]
async fn the_describe_selects_on_the_attached_trust_provider_ids() {
    let (router, engine) = emulator();

    // (case, the trust provider ids attached to this instance).
    let attachments: Vec<(&str, Vec<&str>)> = vec![
        ("no trust provider at all", Vec::new()),
        ("one trust provider", vec!["vatp-0000000000000aaa1"]),
        (
            "two trust providers",
            vec!["vatp-0000000000000bbb2", "vatp-0000000000000ccc3"],
        ),
    ];

    let mut created: Vec<String> = Vec::new();
    for (case, provider_ids) in &attachments {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance_id = guard.insert_verified_access_instance(
            ec2::ops::types::VerifiedAccessInstance {
                description: Some((*case).to_string()),
                verified_access_trust_providers: Some(
                    provider_ids
                        .iter()
                        .map(|provider_id| {
                            ec2::ops::types::VerifiedAccessTrustProviderCondensed {
                                verified_access_trust_provider_id: Some(
                                    (*provider_id).to_string(),
                                ),
                                ..Default::default()
                            }
                        })
                        .collect(),
                ),
                ..Default::default()
            },
            None,
        );
        created.push(instance_id);
    }

    // (case, the trust provider id filtered on, the instances it must select).
    let rows: Vec<(&str, &str, Vec<String>)> = vec![
        (
            "the id of the one attached to a single instance",
            "vatp-0000000000000aaa1",
            vec![created[1].clone()],
        ),
        (
            "the first id of the instance carrying two",
            "vatp-0000000000000bbb2",
            vec![created[2].clone()],
        ),
        (
            "the second id of the instance carrying two",
            "vatp-0000000000000ccc3",
            vec![created[2].clone()],
        ),
        (
            "an id nothing is attached to",
            "vatp-0000000000000ddd4",
            Vec::new(),
        ),
    ];

    for (case, provider_id, expected) in rows {
        assert_eq!(
            described_ids(
                &router,
                case,
                &filters(&[("verified-access-trust-provider-id", &[provider_id])]),
            )
            .await,
            expected,
            "case {case:?}: expected exactly the instances carrying {provider_id:?}"
        );
    }

    // A wildcard over the attached ids selects every instance carrying one at all, and no
    // instance carrying none.
    let mut with_providers = vec![created[1].clone(), created[2].clone()];
    with_providers.sort();
    assert_eq!(
        described_ids(
            &router,
            "a wildcard over the attached ids",
            &filters(&[("verified-access-trust-provider-id", &["vatp-*"])]),
        )
        .await,
        with_providers,
        "a wildcard selects every instance with a trust provider attached, and only those"
    );
}

/// RULE 6's paging: `MaxResults` and `NextToken` walk the collection exactly once, whatever
/// page size the caller asks for, and a token this service did not mint is refused.
#[tokio::test]
async fn the_describe_pages_by_max_results_and_next_token() {
    let (router, _engine) = emulator();

    // More instances than the smallest page the model admits (`range 5..200`), so more than
    // one page is needed at every size under test.
    let mut created: Vec<String> = Vec::new();
    for nth in 0..7 {
        let case = format!("page seed {nth}");
        created.push(seed(&router, &case, &vec![p("Description", case.clone())]).await);
    }
    created.sort();

    // The smallest page the model admits, one in between, exactly the collection, and one
    // more than it.
    for page_size in [5usize, 6, created.len(), created.len() + 1] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(token) = &next_token {
                params.push(p("NextToken", token.clone()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "verifiedAccessInstanceId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults instances, got {}",
                page.len()
            );
            walked.extend(page);
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, created,
            "case {case:?}: paging must walk every instance exactly once, got {walked:?}"
        );
    }

    // No page size named: one page carrying everything the store holds, and no token — the
    // default page is the largest the operation can be asked for, which this suite seeds
    // fewer instances than.
    let (status, body) = describe(&router, &[]).await;
    assert_eq!(
        status, 200,
        "case \"no MaxResults\": expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "verifiedAccessInstanceId"),
        created,
        "case \"no MaxResults\": expected every instance on one page, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "case \"no MaxResults\": a page that holds everything carries no token, got {body}"
    );

    // A token this service did not issue is refused rather than read as a cursor.
    for token in ["not-a-token", "00", ""] {
        let (status, body) = describe(&router, &[p("NextToken", token)]).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a foreign NextToken, got {status} with body \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // A page size outside the modelled `range 5..200` is refused at the boundary, before the
    // handler runs, so only the status is this operation's to promise.
    for (case, max_results) in [
        ("below the modelled minimum", "4"),
        ("zero", "0"),
        ("negative", "-1"),
        ("above the modelled maximum", "201"),
    ] {
        let (status, body) = describe(&router, &[p("MaxResults", max_results)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}
