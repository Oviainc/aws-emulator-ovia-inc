// SPDX-License-Identifier: CC0-1.0

//! The ARNs the S3 Access Grants surface vends, driven end to end over HTTP.
//!
//! The rule under test: a registered location and an access grant are held IN
//! an S3 Access Grants instance, so each is addressed as a subresource OF that
//! instance — the ARN it reports is the instance's own ARN followed by the
//! subresource's type and the id S3 Access Grants assigned it
//! (`<instance-arn>/location/<location-id>`,
//! `<instance-arn>/grant/<grant-id>`) — and every operation that reports one,
//! whether it creates, reads or updates the resource, reports that same ARN.
//!
//! The instance ARN each row builds its expectation from is the one the service
//! itself reports for that instance, so what is pinned here is the RELATION
//! between the two ARNs rather than a second spelling of the instance ARN.
//!
//! Every row below differs along something that rule generalises over: the kind
//! of resource (a registered location and a grant on it), the form of the
//! location scope and so the form of the id the service assigns (the default
//! location, whose id is the documented `default`, against the assigned id of a
//! bucket path and of a bucket-and-prefix path), whether the grant narrows its
//! location, and the account and Region whose instance holds the two — which is
//! the Region every ARN is reported in.
//!
//! Every assertion goes through the same front door a client uses: a buffered
//! `http::Request` dispatched by the multi-service router. The router, the
//! request builder, the XML readers and the calls that create an instance, a
//! location and a grant are the sibling suites', reused rather than copied.

use std::collections::BTreeSet;

use crate::access_grants_data_plane::{
    create_grant, create_instance, create_location, get_grant, get_location,
};
use crate::access_grants_location_update::update_location;
use crate::access_point_lifecycle::{call_in_region, Emulator, Reply, ACCOUNT, REGION};

use service_com_amazonaws_s3control_awss3controlservicev20180820::state::{
    ACCESS_GRANTS_LOCATION_DEFAULT_ID, DEFAULT_LOCATION_SCOPE, PERMISSION_READ,
    PERMISSION_READWRITE,
};

/// A second signing Region, so a row can hold its resources in an instance of
/// another Region: an ARN names the Region its instance lives in.
const OTHER_REGION: &str = "eu-central-1";

/// A third signing Region, for the same reason: a row that needs an instance of
/// its own takes another Region, since a caller is authorized for ONE account.
const THIRD_REGION: &str = "ap-southeast-2";

/// The IAM role an update re-points a location at, so the update reports an ARN
/// for a location whose role is not the one it was registered with. Its shape is
/// the modelled `IAMRoleArn` pattern, `^arn:[^:]+:iam::\d{12}:role/`.
const UPDATED_ROLE: &str = "arn:aws:iam::123456789012:role/access-grants-updated";

/// One row: the instance that holds the resources, the location registered in
/// it, and how the grant on that location narrows it.
struct ArnCase {
    case: &'static str,
    account_id: &'static str,
    region: &'static str,
    location_scope: &'static str,
    /// `Some` narrows the grant to a path inside the location scope; `None`
    /// grants the whole scope.
    sub_prefix: Option<&'static str>,
    permission: &'static str,
}

/// The rows every assertion below is run over.
fn arn_cases() -> Vec<ArnCase> {
    vec![
        ArnCase {
            case: "the default location, whose id is the documented one",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: DEFAULT_LOCATION_SCOPE,
            sub_prefix: Some("arn-bucket/*"),
            permission: PERMISSION_READ,
        },
        ArnCase {
            case: "a whole bucket in the same instance, with a service-assigned id",
            account_id: ACCOUNT,
            region: REGION,
            location_scope: "s3://arn-whole-bucket/",
            sub_prefix: None,
            permission: PERMISSION_READWRITE,
        },
        ArnCase {
            case: "a bucket and prefix in an instance of another Region",
            account_id: ACCOUNT,
            region: OTHER_REGION,
            location_scope: "s3://arn-reports-bucket/reports/",
            sub_prefix: Some("2026/q1.csv"),
            permission: PERMISSION_READ,
        },
        ArnCase {
            case: "an instance in a third Region",
            account_id: ACCOUNT,
            region: THIRD_REGION,
            location_scope: "s3://arn-third-region-bucket/*",
            sub_prefix: None,
            permission: PERMISSION_READWRITE,
        },
    ]
}

/// Read the ARN the service reports for the Access Grants instance `account_id`
/// owns in `region` — the ARN every resource that instance holds is addressed
/// under.
async fn instance_arn(emulator: &Emulator, case: &str, account_id: &str, region: &str) -> String {
    let read = call_in_region(
        &emulator.router,
        region,
        "GET",
        "/v20180820/accessgrantsinstance",
        account_id,
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: expected GetAccessGrantsInstance to answer 200, got {} with body {}",
        read.status, read.body
    );
    read.text("AccessGrantsInstanceArn")
        .unwrap_or_else(|| panic!("case {case:?}: no AccessGrantsInstanceArn in {}", read.body))
}

/// The ARN the resource of type `subresource_type` and id `id` must report: the
/// instance's own ARN, then the type, then the id.
fn expected_subresource_arn(instance_arn: &str, subresource_type: &str, id: &str) -> String {
    format!("{instance_arn}/{subresource_type}/{id}")
}

/// The ARN `reply` reports in `element`, asserted to be `expected`.
fn assert_arn(case: &str, call: &str, reply: &Reply, element: &str, expected: &str) {
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected {call} to answer 200, got {} with body {}",
        reply.status, reply.body
    );
    let reported = reply.text(element).unwrap_or_default();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected {call} to report {element} as {expected:?}, got {reported:?} \
         in body {}",
        reply.body
    );
}

/// Stand an emulator up with an Access Grants instance for every (account,
/// Region) pair the rows use.
async fn emulator_with_instances(cases: &[ArnCase]) -> Emulator {
    let emulator = Emulator::start();
    let mut created = BTreeSet::new();
    for case in cases {
        if created.insert((case.account_id, case.region)) {
            let instance = create_instance(&emulator, case.account_id, case.region).await;
            assert_eq!(
                instance.status, 200,
                "case {:?}: expected CreateAccessGrantsInstance to answer 200, got {} with body {}",
                case.case, instance.status, instance.body
            );
        }
    }
    emulator
}

#[tokio::test]
async fn a_location_is_addressed_as_a_subresource_of_the_instance_that_holds_it() {
    let cases = arn_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        let instance = instance_arn(&emulator, case.case, case.account_id, case.region).await;

        let registered =
            create_location(&emulator, case.account_id, case.region, case.location_scope).await;
        assert_eq!(
            registered.status, 200,
            "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
            case.case, registered.status, registered.body
        );
        let location_id = registered
            .text("AccessGrantsLocationId")
            .unwrap_or_else(|| panic!("case {:?}: no AccessGrantsLocationId", case.case));
        // The id is the documented one for the default location and a
        // service-assigned one otherwise, so the rows span both forms of the
        // last ARN segment.
        if case.location_scope == DEFAULT_LOCATION_SCOPE {
            assert_eq!(
                location_id, ACCESS_GRANTS_LOCATION_DEFAULT_ID,
                "case {:?}: expected the default location's documented id",
                case.case
            );
        }
        let expected = expected_subresource_arn(&instance, "location", &location_id);

        assert_arn(
            case.case,
            "CreateAccessGrantsLocation",
            &registered,
            "AccessGrantsLocationArn",
            &expected,
        );
        assert_arn(
            case.case,
            "GetAccessGrantsLocation",
            &get_location(&emulator, case.account_id, case.region, &location_id).await,
            "AccessGrantsLocationArn",
            &expected,
        );
        // An update changes the location's role, not its identity: it reports
        // the same ARN, and so does a read after it.
        assert_arn(
            case.case,
            "UpdateAccessGrantsLocation",
            &update_location(
                &emulator,
                case.account_id,
                case.region,
                &location_id,
                UPDATED_ROLE,
            )
            .await,
            "AccessGrantsLocationArn",
            &expected,
        );
        assert_arn(
            case.case,
            "GetAccessGrantsLocation after the update",
            &get_location(&emulator, case.account_id, case.region, &location_id).await,
            "AccessGrantsLocationArn",
            &expected,
        );
    }
}

#[tokio::test]
async fn a_grant_is_addressed_as_a_subresource_of_the_instance_that_holds_it() {
    let cases = arn_cases();
    let emulator = emulator_with_instances(&cases).await;

    for case in &cases {
        let instance = instance_arn(&emulator, case.case, case.account_id, case.region).await;
        let registered =
            create_location(&emulator, case.account_id, case.region, case.location_scope).await;
        assert_eq!(
            registered.status, 200,
            "case {:?}: expected CreateAccessGrantsLocation to answer 200, got {} with body {}",
            case.case, registered.status, registered.body
        );
        let location_id = registered
            .text("AccessGrantsLocationId")
            .unwrap_or_else(|| panic!("case {:?}: no AccessGrantsLocationId", case.case));

        let created = create_grant(
            &emulator,
            case.account_id,
            case.region,
            &location_id,
            case.sub_prefix,
            case.permission,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateAccessGrant to answer 200, got {} with body {}",
            case.case, created.status, created.body
        );
        let grant_id = created
            .text("AccessGrantId")
            .unwrap_or_else(|| panic!("case {:?}: no AccessGrantId", case.case));
        let expected = expected_subresource_arn(&instance, "grant", &grant_id);

        assert_arn(
            case.case,
            "CreateAccessGrant",
            &created,
            "AccessGrantArn",
            &expected,
        );
        let read = get_grant(&emulator, case.account_id, case.region, &grant_id).await;
        assert_arn(
            case.case,
            "GetAccessGrant",
            &read,
            "AccessGrantArn",
            &expected,
        );
        // The grant and the location it narrows are addressed under the SAME
        // instance, differing only in the subresource type and the id.
        let location_arn = expected_subresource_arn(&instance, "location", &location_id);
        assert_ne!(
            read.text("AccessGrantArn").unwrap_or_default(),
            location_arn,
            "case {:?}: expected the grant and its location to be addressed as different \
             subresources, got body {}",
            case.case,
            read.body
        );
    }
}
