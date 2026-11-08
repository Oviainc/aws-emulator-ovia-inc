// SPDX-License-Identifier: CC0-1.0

//! `PutBucketNotificationConfiguration`'s DESTINATION VALIDATION: what the call does with
//! the destinations a container names BEFORE it records anything.
//!
//! The rule under test, from this operation's own documentation: "after Amazon S3 receives
//! this request, it first verifies that any Amazon Simple Notification Service (Amazon SNS)
//! or Amazon Simple Queue Service (Amazon SQS) destination exists, and that the bucket
//! owner has permission to publish to it by sending a test notification. In the case of
//! Lambda destinations, Amazon S3 verifies that the Lambda function permissions grant
//! Amazon S3 permission to invoke the function" — and "the PUT notification is an atomic
//! operation … if the message fails, the entire PUT action will fail, and Amazon S3 will
//! not add the configuration to your bucket". So:
//!
//! * a container whose every destination takes that test notification is recorded, and
//!   each queue destination it named has the `s3:TestEvent` waiting on it, naming the
//!   bucket and the request that configured it;
//! * a container naming ONE destination S3 cannot publish to — a queue or a function that
//!   does not exist, a queue S3 cannot publish to at all, a queue whose own access policy
//!   does not let Amazon S3 send to it, a function whose resource-based policy does not let
//!   Amazon S3 invoke it for this bucket — is refused WHOLE, HTTP 400 `InvalidArgument`
//!   with one fixed message and the NUMBERED `<ArgumentName{n}>`/`<ArgumentValue{n}>` pair
//!   naming each failed destination and the reason it failed, and the bucket keeps the
//!   configuration it already carried;
//! * `SkipDestinationValidation` "skips validation of Amazon SQS, Amazon SNS, and Lambda
//!   destinations", so such a request is recorded as given and publishes no test
//!   notification at all.
//!
//! The rows span what the rule generalises over: the destination KIND (an SQS queue, a
//! Lambda function), WHY S3 cannot publish to it (its service does not hold it, it cannot
//! take the publication, its policy grants S3 nothing), WHICH grant its resource policy
//! carries (none, one for this bucket, one for every source, one for another bucket,
//! another principal, another action, another source account), HOW MANY destinations one
//! container names (one, two of one kind, two kinds) and which of them is refused, and
//! whether validation is skipped.
//!
//! Every assertion drives real HTTP requests through a router carrying S3 beside the
//! services that own the destinations, so what is pinned is the wire behaviour a client
//! sees and the message a consumer receives.

use serde_json::json;

use super::object_event_notifications::{
    queue_arn, Emulator, LAMBDA_ROLE, PACKAGE_ZIP_FILE, REGION, XMLNS,
};

/// The error code S3 answers a container it could not validate with, and the status it is
/// rendered with ("InvalidArgument - Invalid Argument", HTTP 400, from Amazon S3's list of
/// error codes).
const INVALID_ARGUMENT: &str = "InvalidArgument";
const INVALID_ARGUMENT_STATUS: u16 = 400;

/// The `<Message>` that refusal carries, as S3 words it.
const UNABLE_TO_VALIDATE: &str = "Unable to validate the following destination configurations";

/// The `Event` of the test notification a validated destination receives.
const TEST_EVENT: &str = "s3:TestEvent";

/// The event every configuration below is notified about — the value is immaterial to the
/// rule, which is about the DESTINATION.
const CONFIGURED_EVENT: &str = "s3:ObjectCreated:*";

/// The event the baseline configuration each bucket starts with is notified about, which is
/// what tells the baseline apart from a row's own container in a read-back.
const BASELINE_EVENT: &str = "s3:ObjectRemoved:*";

/// Which destination kind a row's configuration names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// An SQS queue, as a `QueueConfiguration`.
    Queue,
    /// A Lambda function, as a `CloudFunctionConfiguration`.
    Function,
}

impl Kind {
    /// The element one configuration of this kind is carried in, and the element its ARN is
    /// carried in — the `xmlName`s the model puts on `NotificationConfiguration` and on
    /// each configuration's ARN member
    /// (`smithy_query(query="shape",shape="NotificationConfiguration")`).
    fn elements(self) -> (&'static str, &'static str) {
        match self {
            Self::Queue => ("QueueConfiguration", "Queue"),
            Self::Function => ("CloudFunctionConfiguration", "CloudFunction"),
        }
    }
}

/// How a row's destination exists in the service that owns it when the put is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Existing {
    /// Not created at all, so its own service answers that it does not hold it.
    No,
    /// Created as the ordinary resource an S3 destination is.
    Yes,
    /// Created as a FIFO queue, which takes no publication that carries no
    /// `MessageGroupId` — so it is a queue S3 cannot publish to even though it exists.
    AsFifoQueue,
}

impl Existing {
    /// Whether the resource is created in its own service at all.
    fn created(self) -> bool {
        self != Self::No
    }
}

/// Which grant the destination of a row carries when the put is made — the resource-based
/// policy S3 reads to decide whether it may publish this bucket's notifications to it: the
/// statement `AddPermission` adds to a function, and the `Policy` attribute of a queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grant {
    /// No grant at all: the destination carries no resource policy.
    None,
    /// The action S3 needs, to `s3.amazonaws.com`, narrowed to THIS bucket's ARN.
    ThisBucket,
    /// The same action and principal with no `SourceArn`, so every source is granted.
    AnySource,
    /// The same action and principal, narrowed to ANOTHER bucket's ARN.
    OtherBucket,
    /// The action S3 needs, to another service principal.
    OtherPrincipal,
    /// Another action to `s3.amazonaws.com`.
    OtherAction,
    /// The action S3 needs, to `s3.amazonaws.com`, narrowed to THIS bucket's ARN and to
    /// ANOTHER account as the owner of the publishing bucket. Every part of the grant fits
    /// except the account the notification really comes from, so it grants this bucket
    /// nothing.
    OtherAccount,
}

/// The bucket a [`Grant::OtherBucket`] statement names, which is never the bucket the row
/// configures.
const ANOTHER_BUCKET: &str = "some-other-bucket";

/// The account a [`Grant::OtherAccount`] statement names as the source account: AWS's own
/// documented example account id, which is never the account this emulator serves (every
/// emulator ARN carries `engine::emulator_account_id`).
const ANOTHER_ACCOUNT: &str = "111122223333";

/// One destination a row's container names: its kind, the resource's name, how that
/// resource exists in the owning service before the put is made, the grant its resource
/// policy carries, and — for one the put must refuse — the per-destination DETAIL the
/// refusal has to carry for it.
#[derive(Debug, Clone, Copy)]
struct Named {
    kind: Kind,
    name: &'static str,
    existing: Existing,
    grant: Grant,
    /// `None` for a destination that validates.
    rejected: Option<&'static str>,
}

/// The detail a refusal carries for a destination the service that owns it does not hold —
/// one per destination kind, since only the kind varies in the sentence.
const QUEUE_ABSENT: &str = "The destination queue does not exist";
const FUNCTION_ABSENT: &str = "The destination Lambda function does not exist";

/// The detail it carries for a destination that IS there and whose resource policy does not
/// let Amazon S3 publish this bucket's notifications to it.
const QUEUE_UNPERMITTED: &str = "Permissions on the destination queue do not allow S3 to \
                                 publish notifications from this bucket";
const FUNCTION_UNPERMITTED: &str = "Permissions on the destination Lambda function do not \
                                    allow S3 to publish notifications from this bucket";

/// One row of the table.
struct Row {
    case: &'static str,
    destinations: &'static [Named],
    /// Whether the request carries `SkipDestinationValidation`.
    skip_validation: bool,
    /// The status the put must answer.
    expected_status: u16,
}

/// The rule, over the class it generalises over.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one queue that exists, granting this bucket the publish",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "validated-queue",
                existing: Existing::Yes,
                grant: Grant::ThisBucket,
                rejected: None,
            }],
            skip_validation: false,
            expected_status: 200,
        },
        Row {
            case: "one queue that exists, granting every source the publish",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "any-source-queue",
                existing: Existing::Yes,
                grant: Grant::AnySource,
                rejected: None,
            }],
            skip_validation: false,
            expected_status: 200,
        },
        Row {
            case: "one queue that exists with no policy at all",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "ungranted-queue",
                existing: Existing::Yes,
                grant: Grant::None,
                rejected: Some(QUEUE_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one queue whose policy names another bucket",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "other-bucket-queue",
                existing: Existing::Yes,
                grant: Grant::OtherBucket,
                rejected: Some(QUEUE_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one queue whose policy names another principal",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "other-principal-queue",
                existing: Existing::Yes,
                grant: Grant::OtherPrincipal,
                rejected: Some(QUEUE_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one queue whose policy grants another action",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "other-action-queue",
                existing: Existing::Yes,
                grant: Grant::OtherAction,
                rejected: Some(QUEUE_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one queue that does not exist",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "absent-queue",
                existing: Existing::No,
                grant: Grant::None,
                rejected: Some(QUEUE_ABSENT),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            // Granted the publish and still unable to take it, so what the put reports is
            // that it could not publish to the destination at all.
            case: "a FIFO queue, which exists and takes no publication from S3",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "ordered-queue.fifo",
                existing: Existing::AsFifoQueue,
                grant: Grant::AnySource,
                rejected: Some(QUEUE_ABSENT),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function that exists, granting this bucket the invoke",
            destinations: &[Named {
                kind: Kind::Function,
                name: "validated-function",
                existing: Existing::Yes,
                grant: Grant::ThisBucket,
                rejected: None,
            }],
            skip_validation: false,
            expected_status: 200,
        },
        Row {
            case: "one function that exists, granting every source the invoke",
            destinations: &[Named {
                kind: Kind::Function,
                name: "any-source-function",
                existing: Existing::Yes,
                grant: Grant::AnySource,
                rejected: None,
            }],
            skip_validation: false,
            expected_status: 200,
        },
        Row {
            case: "one function that exists with no invoke permission at all",
            destinations: &[Named {
                kind: Kind::Function,
                name: "ungranted-function",
                existing: Existing::Yes,
                grant: Grant::None,
                rejected: Some(FUNCTION_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function whose invoke grant names another bucket",
            destinations: &[Named {
                kind: Kind::Function,
                name: "other-bucket-function",
                existing: Existing::Yes,
                grant: Grant::OtherBucket,
                rejected: Some(FUNCTION_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function whose invoke grant names another principal",
            destinations: &[Named {
                kind: Kind::Function,
                name: "other-principal-function",
                existing: Existing::Yes,
                grant: Grant::OtherPrincipal,
                rejected: Some(FUNCTION_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function whose grant is of another action",
            destinations: &[Named {
                kind: Kind::Function,
                name: "other-action-function",
                existing: Existing::Yes,
                grant: Grant::OtherAction,
                rejected: Some(FUNCTION_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function whose invoke grant names another source account",
            destinations: &[Named {
                kind: Kind::Function,
                name: "other-account-function",
                existing: Existing::Yes,
                grant: Grant::OtherAccount,
                rejected: Some(FUNCTION_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one queue whose policy names another source account",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "other-account-queue",
                existing: Existing::Yes,
                grant: Grant::OtherAccount,
                rejected: Some(QUEUE_UNPERMITTED),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "one function that does not exist",
            destinations: &[Named {
                kind: Kind::Function,
                name: "absent-function",
                existing: Existing::No,
                grant: Grant::None,
                rejected: Some(FUNCTION_ABSENT),
            }],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "a queue and a function that both exist and both grant it",
            destinations: &[
                Named {
                    kind: Kind::Queue,
                    name: "both-kinds-queue",
                    existing: Existing::Yes,
                    grant: Grant::ThisBucket,
                    rejected: None,
                },
                Named {
                    kind: Kind::Function,
                    name: "both-kinds-function",
                    existing: Existing::Yes,
                    grant: Grant::ThisBucket,
                    rejected: None,
                },
            ],
            skip_validation: false,
            expected_status: 200,
        },
        Row {
            case: "a queue that grants it and a function that grants no invoke",
            destinations: &[
                Named {
                    kind: Kind::Queue,
                    name: "mixed-kinds-queue",
                    existing: Existing::Yes,
                    grant: Grant::ThisBucket,
                    rejected: None,
                },
                Named {
                    kind: Kind::Function,
                    name: "mixed-kinds-ungranted-function",
                    existing: Existing::Yes,
                    grant: Grant::None,
                    rejected: Some(FUNCTION_UNPERMITTED),
                },
            ],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "two queues, the second of which does not exist",
            destinations: &[
                Named {
                    kind: Kind::Queue,
                    name: "partly-valid-queue",
                    existing: Existing::Yes,
                    grant: Grant::ThisBucket,
                    rejected: None,
                },
                Named {
                    kind: Kind::Queue,
                    name: "partly-valid-absent-queue",
                    existing: Existing::No,
                    grant: Grant::None,
                    rejected: Some(QUEUE_ABSENT),
                },
            ],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            // Both refused, so the refusal names BOTH, each with its own reason — which is
            // what the numbered series is for.
            case: "two queues refused for two different reasons",
            destinations: &[
                Named {
                    kind: Kind::Queue,
                    name: "two-reasons-ungranted-queue",
                    existing: Existing::Yes,
                    grant: Grant::None,
                    rejected: Some(QUEUE_UNPERMITTED),
                },
                Named {
                    kind: Kind::Queue,
                    name: "two-reasons-absent-queue",
                    existing: Existing::No,
                    grant: Grant::None,
                    rejected: Some(QUEUE_ABSENT),
                },
            ],
            skip_validation: false,
            expected_status: INVALID_ARGUMENT_STATUS,
        },
        Row {
            case: "a queue that exists, with validation skipped",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "skipped-queue",
                existing: Existing::Yes,
                grant: Grant::None,
                rejected: None,
            }],
            skip_validation: true,
            expected_status: 200,
        },
        Row {
            case: "a queue that does not exist, with validation skipped",
            destinations: &[Named {
                kind: Kind::Queue,
                name: "skipped-absent-queue",
                existing: Existing::No,
                grant: Grant::None,
                rejected: None,
            }],
            skip_validation: true,
            expected_status: 200,
        },
        Row {
            case: "a function granting no invoke, with validation skipped",
            destinations: &[Named {
                kind: Kind::Function,
                name: "skipped-ungranted-function",
                existing: Existing::Yes,
                grant: Grant::None,
                rejected: None,
            }],
            skip_validation: true,
            expected_status: 200,
        },
    ]
}

/// The ARN the destination `named` is addressed by: the forms AWS publishes for each
/// service — `arn:aws:sqs:<region>:<account>:<name>` for a queue and
/// `arn:aws:lambda:<region>:<account>:function:<name>` for a function.
fn destination_arn(named: &Named) -> String {
    match named.kind {
        Kind::Queue => queue_arn(named.name),
        Kind::Function => format!(
            "arn:aws:lambda:{REGION}:{}:function:{}",
            ::engine::emulator_account_id(),
            named.name
        ),
    }
}

/// The `NotificationConfiguration` payload naming `destinations`, each notified about
/// `event`.
///
/// Each configuration filters on a key prefix of its own, which is what lets one container
/// name SEVERAL destinations for one event: two configurations of one event that would both
/// match the same key cannot both be in force, and what this suite is about is the
/// destinations rather than the filters.
fn payload(destinations: &[Named], event: &str) -> String {
    let mut body = format!("<NotificationConfiguration xmlns=\"{XMLNS}\">");
    for named in destinations {
        let (element, arn_element) = named.kind.elements();
        body.push_str(&format!(
            "<{element}><{arn_element}>{arn}</{arn_element}><Event>{event}</Event>\
             <Filter><S3Key><FilterRule><Name>prefix</Name><Value>{prefix}/</Value>\
             </FilterRule></S3Key></Filter></{element}>",
            arn = destination_arn(named),
            prefix = named.name
        ));
    }
    body.push_str("</NotificationConfiguration>");
    body
}

/// The text of every `<name>` element of `xml`, in document order, exactly as the document
/// carries it — the test crate's one reading of a repeated element
/// ([`super::bucket_lifecycle::elements_of`]), so this suite and the harness's own
/// `Answer::elements` cannot read one document two ways.
fn elements(xml: &str, name: &str) -> Vec<String> {
    super::bucket_lifecycle::elements_of(xml, name)
}

/// `PUT /<bucket>?notification` carrying `body`, and `x-amz-skip-destination-validation`
/// when the row asks for it (the header
/// `PutBucketNotificationConfigurationRequest$SkipDestinationValidation` binds to).
async fn put_notification(
    emulator: &Emulator,
    bucket: &str,
    body: &str,
    skip_validation: bool,
) -> super::object_event_notifications::Answer {
    let mut headers = vec![("content-type", "application/xml")];
    if skip_validation {
        headers.push(("x-amz-skip-destination-validation", "true"));
    }
    emulator
        .send(
            "s3",
            "PUT",
            &format!("/{bucket}?notification"),
            &headers,
            body.as_bytes().to_vec(),
        )
        .await
}

/// Create the FIFO queue named `name` — `FifoQueue` is the attribute that makes a queue one
/// ("you can't change the queue type after you create it") — and answer its url.
async fn create_fifo_queue(emulator: &Emulator, name: &str) -> String {
    let created = emulator
        .sqs(
            "CreateQueue",
            &json!({ "QueueName": name, "Attributes": { "FifoQueue": "true" } }),
        )
        .await;
    created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("creating the FIFO queue {name:?} must answer a QueueUrl, got {created}"))
        .to_string()
}

/// The queue's access policy for the statement `grant` describes, or `None` for
/// [`Grant::None`] — the queue then carries no policy at all.
///
/// `sqs:SendMessage` is the action S3 performs on a queue destination, and the grant is
/// narrowed to the publishing bucket by `aws:SourceArn`, which is the condition key AWS
/// documents for a queue policy that admits S3 — an S3 bucket's ARN being
/// `arn:aws:s3:::<bucket>`, with no Region or account "because a bucket name is globally
/// unique".
fn queue_policy(grant: Grant, queue_arn: &str, bucket: &str) -> Option<String> {
    let bucket_arn = |bucket: &str| format!("arn:aws:s3:::{bucket}");
    let mut statement = match grant {
        Grant::None => return None,
        Grant::ThisBucket => json!({
            "Sid": "s3-publish",
            "Effect": "Allow",
            "Principal": {"Service": "s3.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Condition": {"ArnLike": {"aws:SourceArn": bucket_arn(bucket)}},
        }),
        Grant::AnySource => json!({
            "Sid": "s3-publish-any",
            "Effect": "Allow",
            "Principal": {"Service": "s3.amazonaws.com"},
            "Action": "sqs:SendMessage",
        }),
        Grant::OtherBucket => json!({
            "Sid": "s3-publish-other-bucket",
            "Effect": "Allow",
            "Principal": {"Service": "s3.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Condition": {"ArnLike": {"aws:SourceArn": bucket_arn(ANOTHER_BUCKET)}},
        }),
        Grant::OtherPrincipal => json!({
            "Sid": "events-publish",
            "Effect": "Allow",
            "Principal": {"Service": "events.amazonaws.com"},
            "Action": "sqs:SendMessage",
        }),
        Grant::OtherAction => json!({
            "Sid": "s3-receive",
            "Effect": "Allow",
            "Principal": {"Service": "s3.amazonaws.com"},
            "Action": "sqs:ReceiveMessage",
        }),
        // `aws:SourceAccount` is the condition key AWS documents beside `aws:SourceArn` for
        // the account that owns the publishing bucket, so a grant naming another account
        // applies to no notification of this bucket.
        Grant::OtherAccount => json!({
            "Sid": "s3-publish-other-account",
            "Effect": "Allow",
            "Principal": {"Service": "s3.amazonaws.com"},
            "Action": "sqs:SendMessage",
            "Condition": {"ArnLike": {"aws:SourceArn": bucket_arn(bucket)},
                          "StringEquals": {"aws:SourceAccount": ANOTHER_ACCOUNT}},
        }),
    };
    statement["Resource"] = json!(queue_arn);
    Some(json!({ "Version": "2012-10-17", "Statement": [statement] }).to_string())
}

/// Create the Lambda function named `name`, so a configuration naming it resolves, and add
/// the resource-based policy statement `grant` describes.
async fn create_function(emulator: &Emulator, name: &str, bucket: &str, grant: Grant) {
    let created = emulator
        .send(
            "lambda",
            "POST",
            "/2015-03-31/functions",
            &[("content-type", "application/json")],
            json!({
                "FunctionName": name,
                "Role": LAMBDA_ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": { "ZipFile": PACKAGE_ZIP_FILE },
            })
            .to_string()
            .into_bytes(),
        )
        .await;
    assert_eq!(
        created.status,
        201,
        "creating the function {name:?} must succeed, got {} with {}",
        created.status,
        created.text()
    );
    add_permission(emulator, name, bucket, grant).await;
}

/// `AddPermission` on the function named `name` for the statement `grant` describes, or
/// nothing at all for [`Grant::None`].
///
/// The members are the ones `AddPermission` takes — the `Principal` that invokes the
/// function, the `Action` it may use and the `SourceArn` the grant is narrowed to — and an
/// S3 bucket's ARN is `arn:aws:s3:::<bucket>`, which carries no Region or account "because
/// a bucket name is globally unique".
async fn add_permission(emulator: &Emulator, name: &str, bucket: &str, grant: Grant) {
    let bucket_arn = |bucket: &str| format!("arn:aws:s3:::{bucket}");
    let mut statement = match grant {
        Grant::None => return,
        Grant::ThisBucket => json!({
            "StatementId": "s3-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": bucket_arn(bucket),
        }),
        Grant::AnySource => json!({
            "StatementId": "s3-invoke-any",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
        }),
        Grant::OtherBucket => json!({
            "StatementId": "s3-invoke-other-bucket",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": bucket_arn(ANOTHER_BUCKET),
        }),
        Grant::OtherPrincipal => json!({
            "StatementId": "events-invoke",
            "Action": "lambda:InvokeFunction",
            "Principal": "events.amazonaws.com",
            "SourceArn": bucket_arn(bucket),
        }),
        Grant::OtherAction => json!({
            "StatementId": "s3-get-function",
            "Action": "lambda:GetFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": bucket_arn(bucket),
        }),
        // `SourceAccount` is "for Amazon Web Services service principals, the ID of the
        // Amazon Web Services account that owns the resource" (`AddPermission$SourceAccount`):
        // the account that owns the publishing bucket. A grant naming another one fits every
        // part of this notification except where it comes from.
        Grant::OtherAccount => json!({
            "StatementId": "s3-invoke-other-account",
            "Action": "lambda:InvokeFunction",
            "Principal": "s3.amazonaws.com",
            "SourceArn": bucket_arn(bucket),
            "SourceAccount": ANOTHER_ACCOUNT,
        }),
    };
    statement["FunctionName"] = json!(name);
    let granted = emulator
        .send(
            "lambda",
            "POST",
            &format!("/2015-03-31/functions/{name}/policy"),
            &[("content-type", "application/json")],
            statement.to_string().into_bytes(),
        )
        .await;
    assert_eq!(
        granted.status,
        201,
        "granting {grant:?} on the function {name:?} must succeed, got {} with {}",
        granted.status,
        granted.text()
    );
}

#[tokio::test]
async fn a_configuration_is_recorded_only_once_every_destination_it_names_resolves() {
    let emulator = Emulator::new();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let bucket = format!("destination-validation-{index}");
        emulator.create_bucket(&bucket).await;

        // The configuration the bucket carries BEFORE the row's put, so "nothing was
        // recorded" is a read that reports this one rather than one empty container
        // compared against another. It is installed with validation skipped, which is the
        // one way to configure a destination without publishing a test notification.
        let baseline = format!("baseline-{index}");
        let baseline_url = emulator.create_queue(&baseline).await;
        let baseline_body = format!(
            "<NotificationConfiguration xmlns=\"{XMLNS}\"><QueueConfiguration><Queue>{arn}\
             </Queue><Event>{BASELINE_EVENT}</Event></QueueConfiguration>\
             </NotificationConfiguration>",
            arn = queue_arn(&baseline)
        );
        let seeded = put_notification(&emulator, &bucket, &baseline_body, true).await;
        assert_eq!(
            seeded.status,
            200,
            "case {case:?}: seeding the baseline configuration must succeed, got {} with {}",
            seeded.status,
            seeded.text()
        );

        // The destinations this row says exist, created in the services that own them,
        // each carrying the resource policy the row's grant describes. A queue's url
        // travels with it: it is where the row reads the test notification.
        let mut queue_urls: Vec<(&str, String)> = Vec::new();
        for named in row
            .destinations
            .iter()
            .filter(|named| named.existing.created())
        {
            match (named.kind, named.existing) {
                (Kind::Queue, Existing::AsFifoQueue) => {
                    let url = create_fifo_queue(&emulator, named.name).await;
                    if let Some(document) = queue_policy(named.grant, &queue_arn(named.name), &bucket)
                    {
                        emulator.set_queue_policy(&url, &document).await;
                    }
                }
                (Kind::Queue, _) => {
                    let (url, arn) = emulator.create_queue_without_policy(named.name).await;
                    if let Some(document) = queue_policy(named.grant, &arn, &bucket) {
                        emulator.set_queue_policy(&url, &document).await;
                    }
                    queue_urls.push((named.name, url));
                }
                (Kind::Function, _) => {
                    create_function(&emulator, named.name, &bucket, named.grant).await;
                }
            }
        }

        let body = payload(row.destinations, CONFIGURED_EVENT);
        let answer = put_notification(&emulator, &bucket, &body, row.skip_validation).await;
        assert_eq!(
            answer.status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {} with {}",
            row.expected_status,
            answer.status,
            answer.text()
        );

        let read = emulator
            .s3_xml("GET", &format!("/{bucket}?notification"), "")
            .await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: reading the configuration back must succeed, got {} with {}",
            read.status,
            read.text()
        );
        let reported = read.text();

        if row.expected_status == 200 {
            // Recorded: the read reports exactly the destinations the container named,
            // and the baseline it replaced is gone.
            let expected: Vec<String> = row.destinations.iter().map(destination_arn).collect();
            let mut answered = elements(&reported, "Queue");
            answered.extend(elements(&reported, "CloudFunction"));
            assert_eq!(
                answered, expected,
                "case {case:?}: expected the stored destinations {expected:?}, got {answered:?} \
                 in {reported}"
            );

            for (name, url) in &queue_urls {
                if row.skip_validation {
                    // "Skips validation of Amazon SQS … destinations": nothing was sent to
                    // the queue, so the test notification is absent too.
                    let delivered = emulator.nothing_delivered(url).await;
                    assert!(
                        delivered.is_empty(),
                        "case {case:?}: a put that skips destination validation must publish \
                         no test notification, got {delivered:?}"
                    );
                    continue;
                }
                let test_event = emulator.delivered(url).await.unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the queue {name:?} validated must carry the test \
                         notification"
                    )
                });
                assert_eq!(
                    test_event["Event"].as_str(),
                    Some(TEST_EVENT),
                    "case {case:?}: expected the test notification's Event {TEST_EVENT:?}, got \
                     {test_event}"
                );
                assert_eq!(
                    test_event["Service"].as_str(),
                    Some("Amazon S3"),
                    "case {case:?}: expected the test notification to name Amazon S3 as its \
                     service, got {test_event}"
                );
                assert_eq!(
                    test_event["Bucket"].as_str(),
                    Some(bucket.as_str()),
                    "case {case:?}: expected the test notification to name {bucket:?}, got \
                     {test_event}"
                );
                // It names the CALL that configured the bucket, by the pair that call's own
                // response carried.
                let (request_id, host_id) = answer.correlation(case);
                assert_eq!(
                    test_event["RequestId"].as_str(),
                    Some(request_id.as_str()),
                    "case {case:?}: expected the test notification's RequestId {request_id:?}, \
                     got {test_event}"
                );
                assert_eq!(
                    test_event["HostId"].as_str(),
                    Some(host_id.as_str()),
                    "case {case:?}: expected the test notification's HostId {host_id:?}, got \
                     {test_event}"
                );
            }
        } else {
            // Refused: the error document carries the code and the message S3 sends, and
            // the bucket still carries the configuration it had before the put.
            assert!(
                elements(&answer.text(), "Code").contains(&INVALID_ARGUMENT.to_string()),
                "case {case:?}: expected the refusal's Code to be {INVALID_ARGUMENT:?}, got {}",
                answer.text()
            );
            assert!(
                elements(&answer.text(), "Message").contains(&UNABLE_TO_VALIDATE.to_string()),
                "case {case:?}: expected the refusal's Message to be {UNABLE_TO_VALIDATE:?}, \
                 got {}",
                answer.text()
            );

            // That message is the same sentence for every destination that failed, so the
            // document also carries the NUMBERED pair naming each one and the reason it
            // failed, in the order the container named them.
            let expected: Vec<(String, String)> = row
                .destinations
                .iter()
                .filter_map(|named| {
                    named
                        .rejected
                        .map(|detail| (destination_arn(named), detail.to_string()))
                })
                .collect();
            let refused: Vec<(String, String)> = (1..=expected.len())
                .map(|position| {
                    let name = elements(&answer.text(), &format!("ArgumentName{position}"));
                    let value = elements(&answer.text(), &format!("ArgumentValue{position}"));
                    (
                        name.first().cloned().unwrap_or_default(),
                        value.first().cloned().unwrap_or_default(),
                    )
                })
                .collect();
            assert_eq!(
                refused,
                expected,
                "case {case:?}: expected the refusal to name each failed destination and its \
                 reason as {expected:?}, got {refused:?} in {}",
                answer.text()
            );
            assert!(
                elements(
                    &answer.text(),
                    &format!("ArgumentName{}", expected.len() + 1)
                )
                .is_empty(),
                "case {case:?}: the refusal must name no destination past the ones that \
                 failed, got {}",
                answer.text()
            );
            let answered = elements(&reported, "Queue");
            assert_eq!(
                answered,
                vec![queue_arn(&baseline)],
                "case {case:?}: a refused put records nothing, so the bucket must still carry \
                 the baseline configuration, got {reported}"
            );
            assert_eq!(
                elements(&reported, "Event"),
                vec![BASELINE_EVENT.to_string()],
                "case {case:?}: the baseline configuration must be untouched, got {reported}"
            );
            assert!(
                elements(&reported, "CloudFunction").is_empty(),
                "case {case:?}: a refused put records no function configuration, got {reported}"
            );
        }

        // The baseline queue is never a destination of the row's own container, so nothing
        // the row did may have reached it.
        let on_baseline = emulator.nothing_delivered(&baseline_url).await;
        assert!(
            on_baseline.is_empty(),
            "case {case:?}: the baseline queue was configured with validation skipped and is \
             named by no later container, so it must stay empty, got {on_baseline:?}"
        );
    }
}
