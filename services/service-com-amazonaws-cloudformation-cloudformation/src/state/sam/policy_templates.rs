// SPDX-License-Identifier: CC0-1.0

//! The POLICY TEMPLATES a SAM `Policies` entry can name: the shorthand "AWS SAM policy
//! templates" publishes, and the policy document each one stands for.
//!
//! A `Policies` entry is one of three things ("Policies: … This property accepts a single
//! value or a list of values … a list of AWS SAM policy templates", an AWS managed policy
//! name or ARN, or an IAM policy document). The first of those is the shorthand this module
//! answers for: a single-member object whose key is the template's name and whose value
//! states that template's parameters, standing for the policy document the published
//! template grants over the resources those parameters name. [`super`] turns the document
//! into one inline policy on the generated role, so a template entry grants exactly the
//! actions the shorthand names and nothing else.
//!
//! The ARNs a document is granted over are written the way the published templates write
//! them: an `Fn::Sub` whose `${AWS::Partition}`, `${AWS::Region}` and `${AWS::AccountId}`
//! CloudFormation substitutes as it deploys, and whose remaining variable per parameter
//! carries the value the declaration states for it — which may itself be a `Ref` to a
//! resource in the same template, resolved then too. A parameter the declaration leaves out
//! names no resource, so the entry contributes no inline policy rather than one granted over
//! an ARN nothing supplied.
//!
//! [`TEMPLATES`] is the subset of the published catalogue this module carries; a name
//! outside it contributes no inline policy, rather than a document whose actions nothing
//! published.

use ::serde_json::{json, Map, Value};

/// One statement of a template's definition: the actions it allows, and the resources it
/// allows them on.
struct Statement {
    /// The actions the statement allows, as the published template lists them.
    actions: &'static [&'static str],
    /// Each resource the actions are allowed on, as an ARN pattern: `${AWS::…}` is a
    /// pseudo parameter CloudFormation substitutes, and any other `${name}` is this
    /// template's parameter `Name`.
    resources: &'static [&'static str],
}

/// One published policy template: the name a `Policies` entry spells, and the statements
/// its definition carries.
struct PolicyTemplate {
    /// The template's name, which is the entry's single member name.
    name: &'static str,
    /// The statements the template's policy document carries, in the order it states them.
    statements: &'static [Statement],
}

/// The DynamoDB table a `TableName` parameter names, and its indexes — the two resources
/// every table-scoped DynamoDB template is granted over.
const DYNAMODB_TABLE: &[&str] = &[
    "arn:${AWS::Partition}:dynamodb:${AWS::Region}:${AWS::AccountId}:table/${tableName}",
    "arn:${AWS::Partition}:dynamodb:${AWS::Region}:${AWS::AccountId}:table/${tableName}/index/*",
];

/// The bucket a `BucketName` parameter names, and the objects in it — the two resources
/// every bucket-scoped Amazon S3 template is granted over. An S3 bucket ARN carries
/// neither a Region nor an account id.
const S3_BUCKET: &[&str] = &[
    "arn:${AWS::Partition}:s3:::${bucketName}",
    "arn:${AWS::Partition}:s3:::${bucketName}/*",
];

/// Every resource, which is what a template whose actions are not resource-scoped is
/// granted over.
const EVERY_RESOURCE: &[&str] = &["*"];

/// The policy templates this module carries, each as the published catalogue ("AWS SAM
/// policy templates" in the AWS SAM Developer Guide) defines it: its parameters, the
/// actions it allows and the ARNs it allows them on.
const TEMPLATES: &[PolicyTemplate] = &[
    PolicyTemplate {
        name: "DynamoDBCrudPolicy",
        statements: &[Statement {
            actions: &[
                "dynamodb:GetItem",
                "dynamodb:DeleteItem",
                "dynamodb:PutItem",
                "dynamodb:Scan",
                "dynamodb:Query",
                "dynamodb:UpdateItem",
                "dynamodb:BatchWriteItem",
                "dynamodb:BatchGetItem",
                "dynamodb:DescribeTable",
                "dynamodb:ConditionCheckItem",
            ],
            resources: DYNAMODB_TABLE,
        }],
    },
    PolicyTemplate {
        name: "DynamoDBReadPolicy",
        statements: &[Statement {
            actions: &[
                "dynamodb:GetItem",
                "dynamodb:Scan",
                "dynamodb:Query",
                "dynamodb:BatchGetItem",
                "dynamodb:DescribeTable",
            ],
            resources: DYNAMODB_TABLE,
        }],
    },
    PolicyTemplate {
        name: "DynamoDBWritePolicy",
        statements: &[Statement {
            actions: &[
                "dynamodb:PutItem",
                "dynamodb:UpdateItem",
                "dynamodb:BatchWriteItem",
            ],
            resources: DYNAMODB_TABLE,
        }],
    },
    PolicyTemplate {
        name: "DynamoDBStreamReadPolicy",
        statements: &[Statement {
            actions: &[
                "dynamodb:DescribeStream",
                "dynamodb:GetRecords",
                "dynamodb:GetShardIterator",
                "dynamodb:ListStreams",
            ],
            resources: &[
                "arn:${AWS::Partition}:dynamodb:${AWS::Region}:${AWS::AccountId}:table/\
                 ${tableName}/stream/${streamName}",
            ],
        }],
    },
    PolicyTemplate {
        name: "S3ReadPolicy",
        statements: &[Statement {
            actions: &[
                "s3:GetObject",
                "s3:ListBucket",
                "s3:GetBucketLocation",
                "s3:GetObjectVersion",
                "s3:GetLifecycleConfiguration",
            ],
            resources: S3_BUCKET,
        }],
    },
    PolicyTemplate {
        name: "S3WritePolicy",
        statements: &[Statement {
            actions: &[
                "s3:PutObject",
                "s3:PutObjectAcl",
                "s3:PutLifecycleConfiguration",
                "s3:AbortMultipartUpload",
                "s3:ListMultipartUploadParts",
                "s3:ListBucketMultipartUploads",
                "s3:DeleteObject",
            ],
            resources: S3_BUCKET,
        }],
    },
    PolicyTemplate {
        name: "S3CrudPolicy",
        statements: &[Statement {
            actions: &[
                "s3:GetObject",
                "s3:ListBucket",
                "s3:GetBucketLocation",
                "s3:GetObjectVersion",
                "s3:PutObject",
                "s3:PutObjectAcl",
                "s3:GetLifecycleConfiguration",
                "s3:PutLifecycleConfiguration",
                "s3:DeleteObject",
            ],
            resources: S3_BUCKET,
        }],
    },
    PolicyTemplate {
        name: "LambdaInvokePolicy",
        statements: &[Statement {
            actions: &["lambda:InvokeFunction"],
            resources: &[
                "arn:${AWS::Partition}:lambda:${AWS::Region}:${AWS::AccountId}:function:\
                 ${functionName}*",
            ],
        }],
    },
    PolicyTemplate {
        name: "SQSPollerPolicy",
        statements: &[Statement {
            actions: &[
                "sqs:ChangeMessageVisibility",
                "sqs:ChangeMessageVisibilityBatch",
                "sqs:DeleteMessage",
                "sqs:DeleteMessageBatch",
                "sqs:GetQueueAttributes",
                "sqs:ReceiveMessage",
            ],
            resources: &["arn:${AWS::Partition}:sqs:${AWS::Region}:${AWS::AccountId}:${queueName}"],
        }],
    },
    PolicyTemplate {
        name: "SQSSendMessagePolicy",
        statements: &[Statement {
            actions: &["sqs:SendMessage*"],
            resources: &["arn:${AWS::Partition}:sqs:${AWS::Region}:${AWS::AccountId}:${queueName}"],
        }],
    },
    PolicyTemplate {
        name: "SNSPublishMessagePolicy",
        statements: &[Statement {
            actions: &["sns:Publish"],
            resources: &["arn:${AWS::Partition}:sns:${AWS::Region}:${AWS::AccountId}:${topicName}"],
        }],
    },
    PolicyTemplate {
        name: "KinesisStreamReadPolicy",
        statements: &[
            Statement {
                actions: &["kinesis:ListStreams", "kinesis:DescribeLimits"],
                resources: &[
                    "arn:${AWS::Partition}:kinesis:${AWS::Region}:${AWS::AccountId}:stream/*",
                ],
            },
            Statement {
                actions: &[
                    "kinesis:DescribeStream",
                    "kinesis:GetRecords",
                    "kinesis:GetShardIterator",
                ],
                resources: &[
                    "arn:${AWS::Partition}:kinesis:${AWS::Region}:${AWS::AccountId}:stream/\
                     ${streamName}",
                ],
            },
        ],
    },
    PolicyTemplate {
        name: "StepFunctionsExecutionPolicy",
        statements: &[Statement {
            actions: &["states:StartExecution"],
            resources: &[
                "arn:${AWS::Partition}:states:${AWS::Region}:${AWS::AccountId}:stateMachine:\
                 ${stateMachineName}",
            ],
        }],
    },
    PolicyTemplate {
        name: "EventBridgePutEventsPolicy",
        statements: &[Statement {
            actions: &["events:PutEvents"],
            resources: &[
                "arn:${AWS::Partition}:events:${AWS::Region}:${AWS::AccountId}:event-bus/\
                 ${eventBusName}",
            ],
        }],
    },
    PolicyTemplate {
        name: "KMSDecryptPolicy",
        statements: &[Statement {
            actions: &["kms:Decrypt"],
            resources: &["arn:${AWS::Partition}:kms:${AWS::Region}:${AWS::AccountId}:key/${keyId}"],
        }],
    },
    PolicyTemplate {
        name: "AWSSecretsManagerGetSecretValuePolicy",
        statements: &[Statement {
            actions: &["secretsmanager:GetSecretValue"],
            // The secret is named by its ARN, so the statement is granted over the value
            // the declaration states rather than over an ARN built from a name.
            resources: &["${secretArn}"],
        }],
    },
    PolicyTemplate {
        name: "CloudWatchPutMetricPolicy",
        statements: &[Statement {
            actions: &["cloudwatch:PutMetricData"],
            resources: EVERY_RESOURCE,
        }],
    },
    PolicyTemplate {
        name: "VPCAccessPolicy",
        statements: &[Statement {
            actions: &[
                "ec2:CreateNetworkInterface",
                "ec2:DeleteNetworkInterface",
                "ec2:DescribeNetworkInterfaces",
            ],
            resources: EVERY_RESOURCE,
        }],
    },
];

/// The policy document the template `name` grants with the parameters `parameters` states,
/// or `None` when this module carries no template under that name or the declaration leaves
/// out a parameter one of its ARNs is built from.
pub(super) fn document(name: &str, parameters: &Value) -> Option<Value> {
    let template = TEMPLATES.iter().find(|template| template.name == name)?;
    let mut statements = Vec::with_capacity(template.statements.len());
    for statement in template.statements {
        let mut resources = Vec::with_capacity(statement.resources.len());
        for pattern in statement.resources {
            resources.push(granted_over(pattern, parameters)?);
        }
        statements.push(json!({
            "Action": statement.actions,
            "Effect": "Allow",
            // One resource is stated as itself and several as the list of them, which is
            // how a policy document spells either.
            "Resource": match resources.len() {
                1 => resources.remove(0),
                _ => Value::Array(resources),
            },
        }));
    }
    Some(json!({ "Statement": statements }))
}

/// One ARN pattern as the resource a statement is granted over: the pattern itself when it
/// has nothing to substitute, an `Fn::Sub` carrying the template's parameter values when it
/// names any, and the parameter's own value when the pattern is that one parameter.
fn granted_over(pattern: &str, parameters: &Value) -> Option<Value> {
    let variables = parameter_variables(pattern);
    if variables.is_empty() {
        // Nothing of this template's own to substitute: a pseudo parameter is still
        // CloudFormation's to resolve, and a pattern naming none is the ARN itself.
        return Some(match pattern.contains("${") {
            true => json!({ "Fn::Sub": pattern }),
            false => json!(pattern),
        });
    }
    let mut values = Map::new();
    for variable in &variables {
        let value = parameters.get(parameter_name(variable))?;
        values.insert(variable.clone(), value.clone());
    }
    // A pattern that is ONE variable and nothing else is the parameter's value: the
    // declaration already states the whole ARN.
    if variables.len() == 1 && pattern == format!("${{{}}}", variables[0]) {
        return values.into_iter().next().map(|(_, value)| value);
    }
    Some(json!({ "Fn::Sub": [pattern, values] }))
}

/// The variables an ARN pattern names that are this template's own PARAMETERS: every
/// `${name}` in it except the pseudo parameters, which CloudFormation substitutes itself
/// (every pseudo parameter is namespaced, `AWS::Region` and the rest), without repeats.
///
/// The `${…}` form is read by the crate's one reader of it
/// ([`crate::state::template::substitution_names`]), which is also what resolves and
/// validates the `Fn::Sub` this builds — so a pattern here and the substitution it becomes
/// cannot disagree about what a variable is. A name that read an ATTRIBUTE path off a
/// resource (`${Name.Attribute}`) is no parameter of a policy template, so it is not one of
/// these either.
fn parameter_variables(pattern: &str) -> Vec<String> {
    let mut variables: Vec<String> = Vec::new();
    for (name, path) in crate::state::template::substitution_names(pattern) {
        if path.is_none() && !name.contains("::") && !variables.contains(&name) {
            variables.push(name);
        }
    }
    variables
}

/// The parameter a pattern's variable names: the published templates spell a parameter
/// `TableName` as the variable `tableName`, so the parameter is the variable with its
/// first character upper-cased.
fn parameter_name(variable: &str) -> String {
    let mut characters = variable.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pattern in the catalogue names only parameters whose variable spelling the
    /// one naming rule ([`parameter_name`]) can resolve, and no statement is granted over
    /// nothing.
    #[test]
    fn every_template_states_resources_it_can_build() {
        for template in TEMPLATES {
            for statement in template.statements {
                assert!(
                    !statement.actions.is_empty() && !statement.resources.is_empty(),
                    "template {:?}: a statement states both actions and resources",
                    template.name
                );
                for pattern in statement.resources {
                    for variable in parameter_variables(pattern) {
                        let parameter = parameter_name(&variable);
                        assert!(
                            parameter
                                .starts_with(|first: char| first.is_ascii_uppercase()),
                            "template {:?} pattern {pattern:?}: the variable {variable:?} \
                             names the parameter {parameter:?}, which is no parameter name",
                            template.name
                        );
                    }
                }
            }
        }
    }

    /// A name the catalogue does not carry expands to nothing, rather than to a document
    /// nothing published.
    #[test]
    fn an_unknown_template_name_expands_to_nothing() {
        assert!(
            document("NotAPublishedPolicyTemplate", &json!({ "TableName": "items" })).is_none(),
            "a name outside the catalogue carries no document"
        );
    }

    /// A parameter the declaration leaves out leaves the entry unexpanded.
    #[test]
    fn a_missing_parameter_expands_to_nothing() {
        assert!(
            document("DynamoDBCrudPolicy", &json!({})).is_none(),
            "a template whose parameter is not stated carries no document"
        );
    }
}
