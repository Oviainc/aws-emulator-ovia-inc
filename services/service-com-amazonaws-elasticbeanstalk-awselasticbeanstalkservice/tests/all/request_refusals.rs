// SPDX-License-Identifier: CC0-1.0

//! Which code Elastic Beanstalk refuses a request with, where it must not refuse one at
//! all, and which references it resolves before it creates anything.
//!
//! Three rules are asserted here, each over a SET of inputs rather than one recorded
//! request:
//!
//! 1. A value the MODEL refuses — outside an enumerated value set, outside a `@length`
//!    or `@range` bound — is a request-SHAPE failure: HTTP 400 `ValidationError`, in
//!    AWS's `"<N> validation error(s) detected: …"` envelope, raised before the request's
//!    resources are resolved. An absent required member keeps
//!    `MissingRequiredParameter`, and `InvalidParameterValue` stays with the handlers,
//!    for a well-formed value the store cannot use.
//! 2. `DeleteApplication` against a name no application exists under succeeds and
//!    changes nothing, however many times it is called.
//! 3. `CreateEnvironment` resolves a supplied `PlatformArn` against the platform versions
//!    this account holds, and refuses one that resolves to none without persisting an
//!    environment.
//!
//! The emulator harness is `lifecycle.rs`'s: these suites drive the same one service
//! mounted in the production router over its awsQuery HTTP surface.

use service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice as eb;

use crate::harness::{Answer, Emulator, PAST_DUE};
use crate::lifecycle::{account, TEST_REGION};

/// One malformed request, and what the boundary must answer for it.
struct ConstraintCase {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, &'static str)>,
    /// How many MEMBERS the request violates, which is the count AWS's envelope
    /// states ("1 validation error detected", "2 validation errors detected").
    violations: usize,
    /// Fragments the message must carry: the member path the clause names, and the
    /// requirement that member failed.
    fragments: Vec<&'static str>,
}

/// The envelope AWS states a request-validation refusal in.
fn envelope(violations: usize) -> String {
    let noun = if violations == 1 {
        "validation error"
    } else {
        "validation errors"
    };
    format!("{violations} {noun} detected: ")
}

#[tokio::test]
async fn a_value_outside_a_modelled_constraint_is_a_validation_error() {
    let emulator = Emulator::new();

    // Rows span the constraint classes the model can state (enumerated value set,
    // string length, numeric range), the positions a value can fail in (a plain
    // member, one element of a list, several elements of one list, two members at
    // once), and more than one operation. None of them names a resource that
    // exists: a request-shape refusal is raised before identity is resolved, which
    // is exactly what the first row checks by combining a bad enum value with an
    // environment that was never created.
    let cases = vec![
        ConstraintCase {
            case: "enum value outside InstancesHealthAttributes, unknown environment",
            action: "DescribeInstancesHealth",
            params: vec![
                ("EnvironmentName", "ghost-env"),
                ("AttributeNames.member.1", "NotARealAttribute"),
            ],
            violations: 1,
            fragments: vec![
                "Value '[NotARealAttribute]' at 'attributeNames' failed to satisfy constraint: \
                 Member must satisfy constraint: [Member must satisfy enum value set:",
                "HealthStatus",
            ],
        },
        ConstraintCase {
            case: "two enum values outside one list are one member's violation",
            action: "DescribeInstancesHealth",
            params: vec![
                ("AttributeNames.member.1", "NotARealAttribute"),
                ("AttributeNames.member.2", "AlsoNotReal"),
            ],
            violations: 1,
            fragments: vec![
                "Value '[NotARealAttribute, AlsoNotReal]' at 'attributeNames' failed to satisfy \
                 constraint: Member must satisfy constraint: [Member must satisfy enum value set:",
            ],
        },
        ConstraintCase {
            case: "a legal value beside an illegal one still names only the illegal one",
            action: "DescribeInstancesHealth",
            params: vec![
                ("AttributeNames.member.1", "All"),
                ("AttributeNames.member.2", "NotARealAttribute"),
            ],
            violations: 1,
            fragments: vec!["Value '[NotARealAttribute]' at 'attributeNames'"],
        },
        ConstraintCase {
            case: "enum value outside EnvironmentHealthAttributes (another operation)",
            action: "DescribeEnvironmentHealth",
            params: vec![
                ("EnvironmentName", "ghost-env"),
                ("AttributeNames.member.1", "NotAHealthAttribute"),
            ],
            violations: 1,
            fragments: vec![
                "Value '[NotAHealthAttribute]' at 'attributeNames' failed to satisfy constraint: \
                 Member must satisfy constraint: [Member must satisfy enum value set:",
                "InstancesHealth",
            ],
        },
        ConstraintCase {
            case: "string under the modelled minimum length of 4",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "ghost-app"),
                ("EnvironmentName", "ab"),
            ],
            violations: 1,
            fragments: vec![
                "Value 'ab' at 'environmentName' failed to satisfy constraint: Member must have \
                 length greater than or equal to 4",
            ],
        },
        ConstraintCase {
            case: "string over the modelled maximum length of 100",
            action: "CreateApplication",
            params: vec![(
                "ApplicationName",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )],
            violations: 1,
            fragments: vec![
                "at 'applicationName' failed to satisfy constraint: Member must have length less \
                 than or equal to 100",
            ],
        },
        ConstraintCase {
            case: "integer over the modelled maximum of 1000",
            action: "DescribeEnvironments",
            params: vec![("MaxRecords", "1001")],
            violations: 1,
            fragments: vec![
                "at 'maxRecords' failed to satisfy constraint: Member must have value less than \
                 or equal to 1000",
            ],
        },
        ConstraintCase {
            case: "integer under the modelled minimum of 1",
            action: "DescribeEnvironments",
            params: vec![("MaxRecords", "0")],
            violations: 1,
            fragments: vec![
                "at 'maxRecords' failed to satisfy constraint: Member must have value greater \
                 than or equal to 1",
            ],
        },
        ConstraintCase {
            case: "two members violate two different constraints",
            action: "CreateEnvironment",
            params: vec![
                ("ApplicationName", "ghost-app"),
                ("EnvironmentName", "ab"),
                // `Description` is modelled len 0..200; this is 201 characters.
                ("Description", LONG_DESCRIPTION.as_str()),
            ],
            violations: 2,
            fragments: vec!["at 'environmentName'", "at 'description'"],
        },
    ];

    for ConstraintCase {
        case,
        action,
        params,
        violations,
        fragments,
    } in &cases
    {
        let refused = emulator.call(action, params).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "ValidationError",
            "case {case:?}: expected error code ValidationError, got {:?} in body {}",
            refused.error_code(),
            refused.body
        );
        let message = refused.error_message();
        let expected = envelope(*violations);
        assert!(
            message.starts_with(&expected),
            "case {case:?}: expected a message starting {expected:?}, got {message:?}"
        );
        for fragment in fragments {
            assert!(
                message.contains(fragment),
                "case {case:?}: expected the message to carry {fragment:?}, got {message:?}"
            );
        }
    }

    // The other side of the same rule: every value the model DOES declare for
    // `InstancesHealthAttributes` passes the boundary. The catalogue is the
    // service's own modelled value set (`InstancesHealthAttribute`), read from the
    // generated constant rather than written out here, so it cannot drift from the
    // model; what the handler then answers (here, the refusal for an environment
    // that does not exist) is not a request-validation refusal.
    for value in eb::ops::types::INSTANCES_HEALTH_ATTRIBUTE_VALUES {
        let answered = emulator
            .call(
                "DescribeInstancesHealth",
                &[
                    ("EnvironmentName", "ghost-env"),
                    ("AttributeNames.member.1", value),
                ],
            )
            .await;
        assert_ne!(
            answered.error_code(),
            "ValidationError",
            "case {value:?}: a modelled attribute name must not be refused as malformed, \
             got {} and body {}",
            answered.status, answered.body
        );
    }

    // An ABSENT required member is a different refusal, with the code this
    // service's own documentation names for it.
    let absent = emulator
        .call("CreateApplication", &[("Description", "no name supplied")])
        .await;
    assert_eq!(
        absent.status, 400,
        "case \"absent required member\": expected HTTP 400, got {} and body {}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_code(),
        "MissingRequiredParameter",
        "case \"absent required member\": expected MissingRequiredParameter, got body {}",
        absent.body
    );
    assert!(
        absent.error_message().contains("at 'applicationName'"),
        "case \"absent required member\": the message must name the member, got {:?}",
        absent.error_message()
    );

    // A request that omits a required member AND carries an out-of-bound value is
    // refused for the ABSENCE alone: absence is found while READING the request,
    // which stops before the constraint walk over what was read, so the refusal
    // names the absent member and keeps `MissingRequiredParameter`. A refusal
    // mixing the two classes is therefore not reachable from the wire.
    let case = "an absent required member beside a bound violation";
    let mixed = emulator
        .call(
            "CreateApplicationVersion",
            &[("VersionLabel", "v".repeat(101).as_str())],
        )
        .await;
    assert_eq!(
        mixed.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        mixed.status, mixed.body
    );
    assert_eq!(
        mixed.error_code(),
        "MissingRequiredParameter",
        "case {case:?}: expected MissingRequiredParameter, got body {}",
        mixed.body
    );
    assert_eq!(
        mixed.error_message(),
        "Value null at 'applicationName' failed to satisfy constraint: Member must not be null",
        "case {case:?}: the refusal must name the absent member, got {:?}",
        mixed.error_message()
    );
}

/// 201 characters — one past the modelled maximum of `CreateEnvironment$Description`.
static LONG_DESCRIPTION: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| "d".repeat(201));

#[tokio::test]
async fn deleting_an_application_that_does_not_exist_is_a_no_op() {
    let emulator = Emulator::new();

    // The modelled bounds on `ApplicationName` are 1..100 characters, so the rows
    // span the shortest and longest names a caller can send as well as ordinary
    // ones — a delete of a name nothing was ever created under is the same no-op
    // whatever the name looks like.
    let hundred = "n".repeat(100);
    let names = [
        ("never created", "ghost-app".to_string()),
        ("one character, the modelled minimum", "x".to_string()),
        ("one hundred characters, the modelled maximum", hundred),
        ("hyphens and digits", "teardown-app-2".to_string()),
        ("an underscore and a dot", "teardown_app.v2".to_string()),
    ];

    for (case, name) in &names {
        // Twice: the second call is what a re-run teardown makes.
        for attempt in 1..=2 {
            let deleted = emulator
                .call("DeleteApplication", &[("ApplicationName", name)])
                .await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?} attempt {attempt}: expected HTTP 200 for a delete of an \
                 application that does not exist, got {} and body {}",
                deleted.status, deleted.body
            );
            assert_eq!(
                deleted.error_code(),
                "",
                "case {case:?} attempt {attempt}: expected no error envelope, got body {}",
                deleted.body
            );
        }
        // The delete must not have CREATED the application it could not find.
        let applications = emulator.call("DescribeApplications", &[]).await;
        assert!(
            !applications
                .values_at("Applications/member/ApplicationName")
                .contains(&name.as_str()),
            "case {case:?}: the delete must not have created the application, got body {}",
            applications.body
        );
    }

    // The positive control: an application that DOES exist is deleted, and
    // deleting it again is the same no-op. The delete is ASYNCHRONOUS — the
    // application is still described while it runs — so the removal is asserted
    // once the clock has made it due.
    let case = "created then deleted twice";
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "real-app")])
            .await
            .status,
        200,
        "case {case:?}: expected 200 from CreateApplication"
    );
    for attempt in 1..=2 {
        let deleted = emulator
            .call("DeleteApplication", &[("ApplicationName", "real-app")])
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?} attempt {attempt}: expected HTTP 200, got {} and body {}",
            deleted.status, deleted.body
        );
    }
    std::thread::sleep(PAST_DUE);
    let applications = emulator.call("DescribeApplications", &[]).await;
    assert!(
        applications
            .values_at("Applications/member/ApplicationName")
            .is_empty(),
        "case {case:?}: the application must be gone, got body {}",
        applications.body
    );
}

/// The ARN of a custom platform version in this account and region.
fn platform_arn(name: &str, version: &str) -> String {
    format!(
        "arn:aws:elasticbeanstalk:{TEST_REGION}:{}:platform/{name}/{version}",
        account()
    )
}

#[tokio::test]
async fn an_unresolvable_platform_arn_is_refused_and_a_resolvable_one_is_launched() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "platform-app")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );

    // Rows span the forms an unresolvable platform reference arrives in: an
    // AWS-owned platform ARN (no account id), a custom ARN of this account that
    // was never created, the right platform name at a version that was not
    // created, and one created in another region.
    let unresolvable = [
        (
            "an AWS-owned platform ARN naming no published platform",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}::platform/Nonexistent-Platform/9.9.9"
            ),
        ),
        (
            // A published platform branch, at a version the catalog does not publish.
            "a published platform at an unpublished version",
            format!(
                "arn:aws:elasticbeanstalk:{TEST_REGION}::platform/{}/9.9.9",
                eb::state::SOLUTION_STACKS[0].branch_name
            ),
        ),
        (
            "a custom platform ARN never created",
            platform_arn("ghost-platform", "1.0.0"),
        ),
        (
            "a version that was never created",
            platform_arn("sim-platform", "2.0.0"),
        ),
        (
            "a custom platform ARN of another region",
            format!(
                "arn:aws:elasticbeanstalk:eu-west-1:{}:platform/sim-platform/1.0.0",
                account()
            ),
        ),
    ];

    for (index, (case, arn)) in unresolvable.iter().enumerate() {
        let environment_name = format!("plat-env-{index}");
        let refused = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "platform-app"),
                    ("EnvironmentName", &environment_name),
                    ("PlatformArn", arn),
                ],
            )
            .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} and body {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code(),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got body {}",
            refused.body
        );
        assert!(
            refused.error_message().contains(arn.as_str()),
            "case {case:?}: the message must name the platform, got {:?}",
            refused.error_message()
        );
    }
    let environments = emulator
        .call("DescribeEnvironments", &[("ApplicationName", "platform-app")])
        .await;
    assert!(
        environments
            .values_at("Environments/member/EnvironmentId")
            .is_empty(),
        "case \"unresolvable platform\": no environment may have been persisted, got body {}",
        environments.body
    );

    // A platform version this account DID create resolves. It is built before it
    // can be used, so it is `Creating` until the clock makes the build completion
    // due, and an environment cannot be launched on it before then.
    let case = "a created custom platform version";
    let arn = platform_arn("sim-platform", "1.0.0");
    let created = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "sim-platform"),
                ("PlatformVersion", "1.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "my-bucket"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected 200 from CreatePlatformVersion, got {} and body {}",
        created.status, created.body
    );
    assert_eq!(
        created.expect("PlatformSummary/PlatformArn", case),
        arn,
        "case {case:?}: unexpected PlatformArn in {}",
        created.body
    );
    assert_eq!(
        created.expect("PlatformSummary/PlatformStatus", case),
        "Creating",
        "case {case:?}: a new platform version reports Creating, got {}",
        created.body
    );

    // The round trip: the stored version is what DescribePlatformVersion reports.
    let described = describe_platform(&emulator, &arn).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: expected 200 from DescribePlatformVersion, got {} and body {}",
        described.status, described.body
    );
    assert_eq!(
        described.expect("PlatformDescription/PlatformName", case),
        "sim-platform",
        "case {case:?}: the platform name must survive the round trip, got {}",
        described.body
    );
    assert_eq!(
        described.expect("PlatformDescription/PlatformVersion", case),
        "1.0.0",
        "case {case:?}: the platform version must survive the round trip, got {}",
        described.body
    );
    assert_eq!(
        described.expect("PlatformDescription/PlatformOwner", case),
        account(),
        "case {case:?}: a custom platform version is owned by this account, got {}",
        described.body
    );
    assert_eq!(
        described.expect("PlatformDescription/PlatformStatus", case),
        "Creating",
        "case {case:?}: the stored status must be reported, got {}",
        described.body
    );

    let too_early = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "platform-app"),
                ("EnvironmentName", "early-env"),
                ("PlatformArn", &arn),
            ],
        )
        .await;
    assert_eq!(
        too_early.error_code(),
        "InvalidParameterValue",
        "case {case:?}: a platform still being built cannot be launched, got body {}",
        too_early.body
    );

    std::thread::sleep(PAST_DUE);
    let ready = describe_platform(&emulator, &arn).await;
    assert_eq!(
        ready.expect("PlatformDescription/PlatformStatus", case),
        "Ready",
        "case {case:?}: the build the clock owed must have completed, got {}",
        ready.body
    );

    let launched = emulator
        .call(
            "CreateEnvironment",
            &[
                ("ApplicationName", "platform-app"),
                ("EnvironmentName", "plat-env-ok"),
                ("PlatformArn", &arn),
            ],
        )
        .await;
    assert_eq!(
        launched.status, 200,
        "case {case:?}: expected 200 from CreateEnvironment, got {} and body {}",
        launched.status, launched.body
    );
    assert_eq!(
        launched.expect("PlatformArn", case),
        arn,
        "case {case:?}: the environment must report the platform it was launched on, got {}",
        launched.body
    );
    let environments = emulator
        .call("DescribeEnvironments", &[("ApplicationName", "platform-app")])
        .await;
    assert_eq!(
        environments.values_at("Environments/member/PlatformArn"),
        vec![arn.as_str()],
        "case {case:?}: the stored environment must carry the platform ARN, got {}",
        environments.body
    );

    // And a platform version nothing created is not described either.
    let unknown = describe_platform(&emulator, &platform_arn("ghost-platform", "1.0.0")).await;
    assert_eq!(
        unknown.status, 400,
        "case \"describe an unknown platform\": expected HTTP 400, got {} and body {}",
        unknown.status, unknown.body
    );
    assert_eq!(
        unknown.error_code(),
        "InvalidParameterValue",
        "case \"describe an unknown platform\": expected InvalidParameterValue, got body {}",
        unknown.body
    );
}

async fn describe_platform(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("DescribePlatformVersion", &[("PlatformArn", arn)])
        .await
}

#[tokio::test]
async fn a_published_managed_platform_arn_resolves() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator
            .call("CreateApplication", &[("ApplicationName", "managed-app")])
            .await
            .status,
        200,
        "expected 200 from CreateApplication"
    );

    // The rows are the service's OWN published catalog — `SOLUTION_STACKS`, what
    // `ListAvailableSolutionStacks` reports — read from the table rather than
    // written out here: its first, middle and last entries, none of them the
    // platform the issue cited.
    let catalog = eb::state::SOLUTION_STACKS;
    let rows = [
        ("the first published version", catalog[0]),
        ("a published version in the middle", catalog[catalog.len() / 2]),
        ("the last published version", catalog[catalog.len() - 1]),
    ];

    for (index, (case, stack)) in rows.iter().enumerate() {
        let arn = stack.platform_arn(TEST_REGION);
        let described = describe_platform(&emulator, &arn).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: a published platform version must be described, got {} and body {}",
            described.status, described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/PlatformVersion", case),
            stack.platform_version,
            "case {case:?}: unexpected PlatformVersion in {}",
            described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/SolutionStackName", case),
            stack.name(),
            "case {case:?}: the solution stack name must be the catalog's, got {}",
            described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/PlatformOwner", case),
            eb::state::MANAGED_PLATFORM_OWNER,
            "case {case:?}: a managed version is owned by the service, got {}",
            described.body
        );
        assert_eq!(
            described.expect("PlatformDescription/PlatformStatus", case),
            "Ready",
            "case {case:?}: a published version is usable as it stands, got {}",
            described.body
        );

        let environment_name = format!("managed-env-{index}");
        let launched = emulator
            .call(
                "CreateEnvironment",
                &[
                    ("ApplicationName", "managed-app"),
                    ("EnvironmentName", &environment_name),
                    ("PlatformArn", &arn),
                ],
            )
            .await;
        assert_eq!(
            launched.status, 200,
            "case {case:?}: an environment must launch on a published platform, got {} and body {}",
            launched.status, launched.body
        );
        assert_eq!(
            launched.expect("PlatformArn", case),
            arn,
            "case {case:?}: the environment must report the platform it was launched on, got {}",
            launched.body
        );
    }

    // The catalog is per region, so the same published version in another region
    // names nothing in this one.
    let elsewhere = describe_platform(&emulator, &catalog[0].platform_arn("eu-west-1")).await;
    assert_eq!(
        elsewhere.error_code(),
        "InvalidParameterValue",
        "case \"another region's published version\": expected InvalidParameterValue, got body {}",
        elsewhere.body
    );
}

#[tokio::test]
async fn the_platform_version_quota_is_enforced_with_the_modelled_error() {
    let emulator = Emulator::new();
    let case = "one platform version past the bound";

    // Fill the account to its bound through the API, then ask for one more.
    for index in 0..eb::state::MAX_PLATFORM_VERSIONS {
        let created = emulator
            .call(
                "CreatePlatformVersion",
                &[
                    ("PlatformName", "quota-platform"),
                    ("PlatformVersion", &format!("1.0.{index}")),
                    ("PlatformDefinitionBundle.S3Bucket", "my-bucket"),
                    ("PlatformDefinitionBundle.S3Key", "platform.zip"),
                ],
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: version {index} must be created, got {} and body {}",
            created.status, created.body
        );
    }
    let refused = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "quota-platform"),
                ("PlatformVersion", "2.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "my-bucket"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case {case:?}: expected HTTP 400, got {} and body {}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "TooManyPlatformsException",
        "case {case:?}: expected the modelled quota error, got body {}",
        refused.body
    );

    // And creating a version that already exists is refused rather than
    // silently replacing the stored one.
    let duplicate = emulator
        .call(
            "CreatePlatformVersion",
            &[
                ("PlatformName", "quota-platform"),
                ("PlatformVersion", "1.0.0"),
                ("PlatformDefinitionBundle.S3Bucket", "my-bucket"),
                ("PlatformDefinitionBundle.S3Key", "platform.zip"),
            ],
        )
        .await;
    assert_eq!(
        duplicate.error_code(),
        "InvalidParameterValue",
        "case \"a duplicate platform version\": expected InvalidParameterValue, got body {}",
        duplicate.body
    );
}
