// SPDX-License-Identifier: CC0-1.0

//! The FORM an instance id is read in before any lookup, driven over the wire the service
//! speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST answered with XML;
//! README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: EC2 READS A RESOURCE ID IN EITHER OF THE TWO FORMS IT HAS ISSUED
//! IDS IN — a prefix and 8 lowercase hexadecimal digits, or a prefix and 17 of them whose
//! leading digit is `0` — so a value in EITHER form is LOOKED UP, and an account holding no
//! instance for it is answered `InvalidInstanceID.NotFound`, while a value in neither is
//! answered `InvalidInstanceID.Malformed` before any lookup. The two forms are the ones the
//! model's own id patterns publish (`^vpc-([0-9a-f]{8}|[0-9a-f]{17})$`, the pattern quoted
//! on `crate::state::prefixed_id_is_well_formed`); the long form's leading `0` is what
//! AWS's `InvalidInstanceID.Malformed` error-code entry describes as "the full instance ID
//! … in the form `i-xxxxxxxxxxxxxxxxx`", of which EC2 issues only the half with a spare
//! leading digit. The distinction is what a client branches on: a malformed id is a
//! permanent defect in the caller, while a not-found id is a resource that has gone or
//! never existed.
//!
//! The rows span what that generalises over: the NUMBER of digits (both published forms and
//! one digit either side of each), the LEADING digit, the CASE of the digits, the all-zero
//! placeholder, the prefix (none, this kind's, another kind's) and digits that are not
//! hexadecimal — and each row is driven through THREE operations that resolve an instance
//! id: a reporting one, a password read and a mutation, so the reading belongs to the id
//! kind rather than to one handler.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2 as ec2;

use super::ec2_query::{call, element, elements, error_code, error_message};
use super::wire::{param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The image the one launch here names; which image an instance came from has no bearing
/// on how its id is read.
const AMI: &str = "ami-0123456789abcdef0";

/// The code EC2 answers for a value that is not an instance id at all.
const MALFORMED: &str = "InvalidInstanceID.Malformed";

/// The code EC2 answers for an instance id the account holds no instance for.
const NOT_FOUND: &str = "InvalidInstanceID.NotFound";

/// One operation that resolves an instance id: the action, the form fields that name one
/// id and nothing else of consequence, and whether its OUTPUT reports the id it resolved —
/// a reboot answers the envelope's success flag and no id of its own.
struct Resolver {
    action: &'static str,
    fields: fn(&str) -> Vec<Param>,
    reports_the_id: bool,
}

/// The operations each row is driven through — a describe, a password read and a reboot, so
/// a reporting path and a mutating path are both covered.
///
/// The reboot is state-independent for the instance launched below: it acts on an instance
/// that is `running` or `pending` alike (`src/ops/reboot_instances.rs`), so the launch's own
/// transition window cannot make these rows timing-sensitive.
const RESOLVERS: [Resolver; 3] = [
    Resolver {
        action: "DescribeInstances",
        fields: |instance_id| vec![param("InstanceId.1", instance_id)],
        reports_the_id: true,
    },
    Resolver {
        action: "GetPasswordData",
        fields: |instance_id| vec![param("InstanceId", instance_id)],
        reports_the_id: true,
    },
    Resolver {
        action: "RebootInstances",
        fields: |instance_id| vec![param("InstanceId.1", instance_id)],
        reports_the_id: false,
    },
];

/// One row: what is notable about the value, the value, and the code every operation above
/// must refuse it with.
struct Row {
    case: &'static str,
    instance_id: &'static str,
    code: &'static str,
}

/// Every row of the id-form table.
///
/// The two published forms are each covered by a value no instance holds (so the answer is
/// the not-found half of the rule) and are bracketed by a value one digit short and one
/// digit past them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the short form, eight digits under a leading digit above zero",
            instance_id: "i-9f3c71de",
            code: NOT_FOUND,
        },
        Row {
            case: "the short form, eight digits under a leading zero",
            instance_id: "i-0f3c71de",
            code: NOT_FOUND,
        },
        Row {
            case: "one digit short of the short form",
            instance_id: "i-9f3c71d",
            code: MALFORMED,
        },
        Row {
            case: "one digit past the short form",
            instance_id: "i-9f3c71de0",
            code: MALFORMED,
        },
        Row {
            case: "the all-zero short id, which the mint counting from one never produces",
            instance_id: "i-00000000",
            code: MALFORMED,
        },
        Row {
            case: "the long form, seventeen digits under a leading zero",
            instance_id: "i-0a1b2c3d4e5f60718",
            code: NOT_FOUND,
        },
        Row {
            case: "the long form at its largest under a leading zero",
            instance_id: "i-0ffffffffffffffff",
            code: NOT_FOUND,
        },
        Row {
            case: "the long form with a leading digit above zero, which EC2 does not issue",
            instance_id: "i-9a1b2c3d4e5f60718",
            code: MALFORMED,
        },
        Row {
            case: "one digit short of the long form",
            instance_id: "i-0a1b2c3d4e5f6071",
            code: MALFORMED,
        },
        Row {
            case: "one digit past the long form",
            instance_id: "i-0a1b2c3d4e5f607182",
            code: MALFORMED,
        },
        Row {
            case: "the all-zero long id, the placeholder every client reaches for",
            instance_id: "i-00000000000000000",
            code: MALFORMED,
        },
        Row {
            case: "upper-case hexadecimal digits, which no issued id carries",
            instance_id: "i-0A1B2C3D",
            code: MALFORMED,
        },
        Row {
            case: "a digit that is not hexadecimal",
            instance_id: "i-0a1b2c3z",
            code: MALFORMED,
        },
        Row {
            case: "the prefix with no digits at all",
            instance_id: "i-",
            code: MALFORMED,
        },
        Row {
            case: "digits in a published form under no prefix",
            instance_id: "0a1b2c3d",
            code: MALFORMED,
        },
        Row {
            case: "digits in a published form under another resource kind's prefix",
            instance_id: "vol-0a1b2c3d4e5f60718",
            code: MALFORMED,
        },
        Row {
            case: "a prefix that spells the resource out rather than naming its kind",
            instance_id: "instance-0a1b2c3d",
            code: MALFORMED,
        },
        Row {
            case: "free text",
            instance_id: "not-an-instance",
            code: MALFORMED,
        },
    ]
}

/// Every value outside the two published forms is refused as malformed, and every value
/// inside them that names no instance is refused as not found — on each of the three
/// operations, with the sentence that half of the rule carries.
#[tokio::test]
async fn an_instance_id_is_read_in_either_published_form_before_it_is_looked_up() {
    let (router, _engine) = super::ec2_query::emulator();
    // One live instance, so the refusals below come from the id in the request rather than
    // from an account with nothing in it.
    let launched = launch(&router).await;

    for row in rows() {
        for resolver in &RESOLVERS {
            let case = format!("{} ({})", row.case, resolver.action);
            let (status, body) = call(
                &router,
                REGION,
                resolver.action,
                &(resolver.fields)(row.instance_id),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?} ({:?}): expected HTTP 400, got {status} with body {body}",
                row.instance_id
            );
            assert_eq!(
                error_code(&body),
                row.code,
                "case {case:?} ({:?}): expected {}, got {body}",
                row.instance_id,
                row.code
            );
            // Which sentence the refusal carries is the other half of telling the two
            // apart: the not-found half names the id as an instance ID, the malformed half
            // quotes the value back as one the id space could not read.
            let expected_message = if row.code == NOT_FOUND {
                format!("The instance ID '{}' does not exist", row.instance_id)
            } else {
                format!("Invalid id: \"{}\"", row.instance_id)
            };
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?} ({:?}): expected the message {expected_message:?}, got {body}",
                row.instance_id
            );
            // A refusal is correlatable: the ec2Query error envelope closes with the id of
            // the request it answers, whichever half of the rule refused it.
            let request_id = element(&body, "RequestID");
            assert!(
                request_id.as_deref().is_some_and(|id| !id.is_empty()),
                "case {case:?} ({:?}): the refusal must close with a non-empty <RequestID>, \
                 got {body}",
                row.instance_id
            );
        }
    }

    // The instance that does exist still resolves on every one of those operations: the
    // refusals above are about the ids, not about the operations.
    for resolver in &RESOLVERS {
        let case = format!("the launched instance ({})", resolver.action);
        let (status, body) = call(
            &router,
            REGION,
            resolver.action,
            &(resolver.fields)(&launched),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the launched instance must still resolve, got {status} with \
             body {body}"
        );
        if resolver.reports_the_id {
            assert_eq!(
                elements(&body, "instanceId"),
                vec![launched.clone()],
                "case {case:?}: the answer must name the instance the service resolved, \
                 got {body}"
            );
        }
    }
}

/// The id a launch mints is in the long published form, and it reads back through the
/// describe — the round trip the form rule is about.
#[tokio::test]
async fn a_minted_instance_id_is_in_the_long_published_form_and_reads_back() {
    let (router, _engine) = super::ec2_query::emulator();
    let instance_id = launch(&router).await;

    let suffix = instance_id
        .strip_prefix("i-")
        .unwrap_or_else(|| panic!("a minted instance id must carry the `i-` prefix: {instance_id}"));
    assert_eq!(
        suffix.len(),
        ec2::state::LONG_RESOURCE_ID_DIGITS,
        "case {instance_id:?}: a minted id must carry the long form's {} digits, got {}",
        ec2::state::LONG_RESOURCE_ID_DIGITS,
        suffix.len()
    );
    assert!(
        suffix.starts_with('0')
            && suffix
                .bytes()
                .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f')),
        "case {instance_id:?}: a minted id must be lowercase hexadecimal under a leading \
         zero, got {suffix:?}"
    );

    let (status, body) = call(
        &router,
        REGION,
        "DescribeInstances",
        &[param("InstanceId.1", &instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {instance_id:?}: the describe of the minted id must succeed, got {status} with \
         body {body}"
    );
    assert_eq!(
        elements(&body, "instanceId"),
        vec![instance_id.clone()],
        "case {instance_id:?}: the describe must report exactly the instance the launch \
         minted, got {body}"
    );
}

/// One instance, launched, as its id.
async fn launch(router: &MultiServiceRouter) -> String {
    let (status, body) = call(
        router,
        REGION,
        "RunInstances",
        &[
            param("ImageId", AMI),
            param("MinCount", "1"),
            param("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the launch must succeed, got {status} with body {body}"
    );
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        1,
        "the launch must report exactly one instance id, got {ids:?}"
    );
    ids.into_iter().next().unwrap_or_default()
}
