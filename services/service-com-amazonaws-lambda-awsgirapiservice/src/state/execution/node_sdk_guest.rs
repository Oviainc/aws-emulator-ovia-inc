// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a node handler imports: the `@aws-sdk/client-*` packages this runtime
//! supplies on the handler's resolver path, in JavaScript, over the one host function that
//! makes a call ([`super::aws_api`]).
//!
//! AWS Lambda's managed `nodejs18.x` and later runtimes ship the AWS SDK for JavaScript v3
//! in the runtime's own `node_modules`, so `require('@aws-sdk/client-s3')` resolves with no
//! dependency in the deployment package and the handler can call another service. These
//! modules are that directory's contents here. They are resolved LAST — after the
//! package's own `node_modules` and after the layers' — which is the order a managed
//! runtime resolves them in, so a function that ships its own copy of a client keeps it.
//!
//! What they do and do not do:
//!
//! * The module for `@aws-sdk/client-<token>` exports a `<Token>Client` whose `send`
//!   dispatches ONE call, and a command class for every `…Command` name asked of it. The
//!   command's `input` IS the operation's input document, which is what an AWS JSON
//!   protocol puts on the wire; the host builds, signs and dispatches the request
//!   ([`super::aws_api`]) and the answer's document is what `send` resolves with.
//! * A refusal is thrown as an error whose `name` is the service's own error code, with the
//!   `$metadata` the answer carried — the shape a v3 client throws — so a handler that
//!   catches one reads the emulated service's own answer.
//! * A service whose protocol this seam does not bind, and an operation name that is not
//!   one, are REFUSED: the call is reported as not made rather than answered with a
//!   made-up success.
//! * Nothing here is the containment. Which services can be reached, the bounds on a call
//!   and the signature it carries are all host code's ([`super::aws_api`], [`super::node`]).

use super::package::PackageModule;

/// The prefix of the package names this runtime answers for, which is the one namespace the
/// AWS SDK for JavaScript v3 publishes its service clients under.
const CLIENT_PACKAGE_PREFIX: &str = "@aws-sdk/client-";

/// The modules this runtime puts on a node handler's resolver path, under the ids the
/// runtime's own `node_modules` is keyed by.
///
/// The client packages themselves are not listed: the SDK publishes one package per AWS
/// service and a managed runtime bundles them all, so the package for any
/// `@aws-sdk/client-<token>` is answered by [`runtime_module_source`] rather than written
/// out here.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    Vec::new()
}

/// The source of the runtime-supplied module `id` names, or `None` when this runtime
/// supplies none.
///
/// `id` is the id of an entry of the runtime's own `node_modules`, so the name asked for is
/// the package name a bare specifier resolved to.
#[must_use]
pub fn runtime_module_source(id: &str) -> Option<String> {
    let package = id
        .strip_prefix(super::node::RUNTIME_ID_PREFIX)?
        .strip_prefix("node_modules/")?;
    if package == DOCUMENT_CLIENT_PACKAGE {
        return Some(DOCUMENT_CLIENT_SOURCE.to_string());
    }
    if package == DYNAMODB_UTIL_PACKAGE {
        return Some(DYNAMODB_UTIL_SOURCE.to_string());
    }
    let token = package.strip_prefix(CLIENT_PACKAGE_PREFIX)?;
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return None;
    }
    Some(CLIENT_SOURCE.replace("__SERVICE_TOKEN__", token).replace("__CLIENT_NAME__", &client_name(token)))
}

/// The class the `@aws-sdk/client-<token>` package exports its client under.
///
/// The SDK names it after the service's own SDK id with the spaces dropped — `S3Client`,
/// `DynamoDBClient` — and the package name is that id lower-cased, which loses the casing.
/// The services whose casing is not the plain one are listed here, by the name their own
/// package exports; every other token is the name with its first letter upper-cased, which
/// is what the SDK's own naming gives for a single-word id.
fn client_name(token: &str) -> String {
    const EXPORTED: &[(&str, &str)] = &[
        ("s3", "S3"),
        ("dynamodb", "DynamoDB"),
        ("dynamodbstreams", "DynamoDBStreams"),
        ("sqs", "SQS"),
        ("sns", "SNS"),
        ("sts", "STS"),
        ("ssm", "SSM"),
        ("kms", "KMS"),
        ("iam", "IAM"),
        ("ec2", "EC2"),
        ("ecs", "ECS"),
        ("ecr", "ECR"),
        ("eks", "EKS"),
        ("efs", "EFS"),
        ("rds", "RDS"),
        ("sfn", "SFN"),
        ("ses", "SES"),
        ("sesv2", "SESv2"),
        ("eventbridge", "EventBridge"),
        ("cloudwatch", "CloudWatch"),
        ("cloudwatch-logs", "CloudWatchLogs"),
        ("cloudwatch-events", "CloudWatchEvents"),
        ("cloudformation", "CloudFormation"),
        ("cloudfront", "CloudFront"),
        ("api-gateway", "APIGateway"),
        ("apigatewayv2", "ApiGatewayV2"),
        ("secrets-manager", "SecretsManager"),
        ("step-functions", "SFN"),
        ("service-catalog", "ServiceCatalog"),
        ("cognito-identity", "CognitoIdentity"),
        ("cognito-identity-provider", "CognitoIdentityProvider"),
    ];
    if let Some((_, named)) = EXPORTED.iter().find(|(package, _)| *package == token) {
        return format!("{named}Client");
    }
    let mut characters = token.chars();
    let head = characters
        .next()
        .map(|first| first.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    format!("{head}{}Client", characters.as_str().replace('-', ""))
}

/// The package the SDK publishes the DynamoDB document client under.
const DOCUMENT_CLIENT_PACKAGE: &str = "@aws-sdk/lib-dynamodb";

/// The package the SDK publishes the DynamoDB value conversions under.
const DYNAMODB_UTIL_PACKAGE: &str = "@aws-sdk/util-dynamodb";

/// One service client package: the client, and a command class for every operation name
/// asked of it.
///
/// The service token is the package's own name — the one the SDK publishes that service's
/// client under — and it is what the host resolves the destination from.
const CLIENT_SOURCE: &str = r##"
'use strict';

var SERVICE = '__SERVICE_TOKEN__';

function host() {
    return globalThis.__lambdaRuntime;
}

// `new <Op>Command(input)`: the operation this command addresses is its own class name
// without the `Command` suffix, which is how a v3 command names its operation.
function commandClass(name) {
    var operation = name.slice(0, name.length - 'Command'.length);
    var Command = function (input) {
        this.input = input === undefined || input === null ? {} : input;
    };
    Command.prototype.operation = operation;
    Object.defineProperty(Command, 'name', { value: name });
    return Command;
}

// The error a v3 client throws for a service's refusal: the service's own error code as
// the error's name, its message, and the metadata of the answer that carried them.
function serviceError(answered) {
    var document = {};
    try {
        document = JSON.parse(answered.body);
    } catch (error) {
        document = {};
    }
    var code = answered.errorType === null || answered.errorType === undefined
        ? (document.__type === undefined ? 'ServiceException' : document.__type)
        : answered.errorType;
    // An error shape travels as `<shape>` or `<namespace>#<shape>`; the name a client
    // reports is the shape's own.
    var named = String(code).split('#').pop().split(':')[0];
    var message = document.message === undefined
        ? (document.Message === undefined ? '' : document.Message)
        : document.message;
    var error = new Error(message);
    error.name = named;
    error.$metadata = {
        httpStatusCode: answered.status,
        requestId: answered.requestId === undefined ? undefined : answered.requestId
    };
    error.$fault = answered.status >= 500 ? 'server' : 'client';
    return error;
}

// The input members a call carries BYTES in, by service: every protocol this SDK speaks
// carries a blob as base64 text, so the bytes a handler passes in one of these are encoded
// before the call is made. A string is encoded as UTF-8 first, which is what a client does
// with a text body.
var BLOB_PARAMETERS = { s3: ['Body'] };

function sentInput(input) {
    var members = BLOB_PARAMETERS[SERVICE];
    if (members === undefined || input === null || typeof input !== 'object') {
        return input;
    }
    var sent = {};
    Object.keys(input).forEach(function (name) { sent[name] = input[name]; });
    members.forEach(function (member) {
        if (sent[member] === undefined || sent[member] === null) {
            return;
        }
        var value = sent[member];
        if (typeof value === 'string') {
            value = globalThis.Buffer.from(value, 'utf8');
        }
        sent[member] = globalThis.Buffer.from(value).toString('base64');
    });
    return sent;
}

// A member of an answer that carries BYTES is handed over the way a v3 client hands over a
// streamed body: the bytes are already here, so the two transforms resolve at once.
function streamedBody(base64Text) {
    var bytes = new Uint8Array(globalThis.Buffer.from(base64Text, 'base64'));
    return {
        transformToByteArray: function () { return Promise.resolve(bytes); },
        transformToString: function (encoding) {
            return Promise.resolve(
                globalThis.Buffer.from(bytes).toString(
                    encoding === undefined ? 'utf8' : encoding
                )
            );
        }
    };
}

function notMade(answered) {
    var error = new Error(answered.detail === undefined || answered.detail === null
        ? 'The request was not made: ' + answered.outcome
        : answered.detail);
    error.name = 'UnrecognizedClientException';
    if (answered.outcome === 'unknown_service') {
        error.name = 'NotImplemented';
        error.message = 'This emulator makes no call to ' + SERVICE
            + '; the services it reaches are: ' + answered.detail;
    }
    error.$metadata = {};
    return error;
}

function Client(configuration) {
    var held = configuration === undefined || configuration === null ? {} : configuration;
    this.config = {
        region: held.region === undefined ? globalThis.process.env.AWS_REGION : held.region,
        serviceId: SERVICE
    };
}

Client.prototype.send = function (command) {
    var self = this;
    return new Promise(function (resolve, reject) {
        var operation = command === null || command === undefined ? undefined : command.operation;
        if (typeof operation !== 'string') {
            reject(new TypeError('The argument must be a command of this client'));
            return;
        }
        var region = typeof self.config.region === 'string' ? self.config.region : '';
        var answered;
        try {
            answered = JSON.parse(host().awsCall(
                SERVICE,
                operation,
                region,
                JSON.stringify(sentInput(command.input === undefined ? {} : command.input))
            ));
        } catch (error) {
            reject(error);
            return;
        }
        if (answered.outcome !== 'answered') {
            reject(notMade(answered));
            return;
        }
        if (answered.status >= 400) {
            reject(serviceError(answered));
            return;
        }
        var document = {};
        try {
            document = answered.body === '' ? {} : JSON.parse(answered.body);
        } catch (error) {
            document = {};
        }
        var streams = answered.streams === undefined || answered.streams === null
            ? []
            : answered.streams;
        streams.forEach(function (member) {
            if (typeof document[member] === 'string') {
                document[member] = streamedBody(document[member]);
            }
        });
        document.$metadata = {
            httpStatusCode: answered.status,
            requestId: answered.requestId === undefined ? undefined : answered.requestId
        };
        resolve(document);
    });
};

Client.prototype.destroy = function () { return undefined; };

// The client's own exported name is the service's token in the form the SDK names it:
// `S3Client` for `client-s3`, `DynamoDBClient` for `client-dynamodb`, which is the name a
// handler destructures from this package. A name a handler asks for that this module does
// not hold a value for is answered as the kind of member it names — a command class for a
// `…Command`, the client for a `…Client` — and as nothing otherwise, so a typo is
// `undefined` rather than a stand-in.
var members = Object.create(null);
members.__CLIENT_NAME__ = Client;

module.exports = new Proxy(members, {
    get: function (target, name) {
        if (typeof name !== 'string') {
            return target[name];
        }
        if (Object.prototype.hasOwnProperty.call(target, name)) {
            return target[name];
        }
        if (name.length > 'Command'.length
            && name.slice(name.length - 'Command'.length) === 'Command') {
            target[name] = commandClass(name);
            return target[name];
        }
        if (name.length > 'Client'.length
            && name.slice(name.length - 'Client'.length) === 'Client') {
            target[name] = Client;
            return target[name];
        }
        return undefined;
    },
    has: function (target, name) {
        return typeof name === 'string'
            && (Object.prototype.hasOwnProperty.call(target, name)
                || name.slice(name.length - 'Command'.length) === 'Command'
                || name.slice(name.length - 'Client'.length) === 'Client');
    }
});
"##;

/// `@aws-sdk/util-dynamodb`: the two conversions between a plain JavaScript value and the
/// attribute value DynamoDB's own wire form carries it as.
const DYNAMODB_UTIL_SOURCE: &str = r##"
'use strict';

// The attribute types DynamoDB names a value by: the scalar ones, the sets, and the two
// document types.
function convertToAttr(value, options) {
    var held = options === undefined || options === null ? {} : options;
    if (value === null) {
        return { NULL: true };
    }
    if (value === undefined) {
        if (held.removeUndefinedValues === true) {
            return undefined;
        }
        throw new Error('Pass options.removeUndefinedValues=true to remove undefined values');
    }
    if (typeof value === 'boolean') {
        return { BOOL: value };
    }
    if (typeof value === 'number') {
        if (!isFinite(value)) {
            throw new Error('Special numeric value ' + value + ' is not allowed');
        }
        return { N: String(value) };
    }
    if (typeof value === 'bigint') {
        return { N: value.toString() };
    }
    if (typeof value === 'string') {
        if (value === '' && held.convertEmptyValues === true) {
            return { NULL: true };
        }
        return { S: value };
    }
    if (value instanceof Uint8Array) {
        return { B: globalThis.Buffer.from(value).toString('base64') };
    }
    if (value instanceof Set) {
        return setAttribute(value, held);
    }
    if (Array.isArray(value)) {
        return {
            L: value
                .map(function (member) { return convertToAttr(member, held); })
                .filter(function (member) { return member !== undefined; })
        };
    }
    if (typeof value === 'object') {
        var map = {};
        var keys = Object.keys(value);
        for (var index = 0; index < keys.length; index += 1) {
            var converted = convertToAttr(value[keys[index]], held);
            if (converted !== undefined) {
                map[keys[index]] = converted;
            }
        }
        return { M: map };
    }
    throw new Error('Unsupported type passed: ' + typeof value);
}

// A Set is one of the three typed sets, decided by what its members are — which is the
// only thing that decides it, because DynamoDB has no mixed set type.
function setAttribute(value, options) {
    var members = Array.prototype.slice.call(value);
    if (members.length === 0) {
        if (options.convertEmptyValues === true) {
            return { NULL: true };
        }
        throw new Error('Pass a non-empty set, or options.convertEmptyValues=true');
    }
    if (members.every(function (member) { return typeof member === 'string'; })) {
        return { SS: members };
    }
    if (members.every(function (member) {
        return typeof member === 'number' || typeof member === 'bigint';
    })) {
        return { NS: members.map(String) };
    }
    if (members.every(function (member) { return member instanceof Uint8Array; })) {
        return {
            BS: members.map(function (member) {
                return globalThis.Buffer.from(member).toString('base64');
            })
        };
    }
    throw new Error('Only Number Set (NS), Binary Set (BS) or String Set (SS) are allowed');
}

function convertToNative(attribute, options) {
    var held = options === undefined || options === null ? {} : options;
    if (attribute === null || typeof attribute !== 'object') {
        throw new Error('No value defined: ' + attribute);
    }
    if (attribute.NULL !== undefined) {
        return null;
    }
    if (attribute.BOOL !== undefined) {
        return attribute.BOOL;
    }
    if (attribute.S !== undefined) {
        return attribute.S;
    }
    if (attribute.N !== undefined) {
        return held.wrapNumbers === true ? { value: attribute.N } : Number(attribute.N);
    }
    if (attribute.B !== undefined) {
        return new Uint8Array(globalThis.Buffer.from(attribute.B, 'base64'));
    }
    if (attribute.SS !== undefined) {
        return new Set(attribute.SS);
    }
    if (attribute.NS !== undefined) {
        return new Set(attribute.NS.map(Number));
    }
    if (attribute.BS !== undefined) {
        return new Set(attribute.BS.map(function (member) {
            return new Uint8Array(globalThis.Buffer.from(member, 'base64'));
        }));
    }
    if (attribute.L !== undefined) {
        return attribute.L.map(function (member) { return convertToNative(member, held); });
    }
    if (attribute.M !== undefined) {
        var out = {};
        var keys = Object.keys(attribute.M);
        for (var index = 0; index < keys.length; index += 1) {
            out[keys[index]] = convertToNative(attribute.M[keys[index]], held);
        }
        return out;
    }
    throw new Error('Unsupported type passed: ' + Object.keys(attribute).join(', '));
}

function marshall(value, options) {
    var converted = convertToAttr(value, options);
    return converted === undefined || converted.M === undefined
        ? converted
        : converted.M;
}

function unmarshall(item, options) {
    return convertToNative({ M: item === undefined || item === null ? {} : item }, options);
}

exports.convertToAttr = convertToAttr;
exports.convertToNative = convertToNative;
exports.marshall = marshall;
exports.unmarshall = unmarshall;
exports.NumberValue = function (value) { return { value: String(value) }; };
"##;

/// `@aws-sdk/lib-dynamodb`: the DOCUMENT client, which is the low-level client with the
/// attribute-value conversion applied to the members that carry items.
const DOCUMENT_CLIENT_SOURCE: &str = r##"
'use strict';

var conversions = require('@aws-sdk/util-dynamodb');

// The operation each document command addresses: the command's own name without the
// `Command` suffix, mapped to the low-level operation of the same action. The four
// single-item actions are named `…Item` on the wire and the batch and transaction ones
// carry their own suffix, which is the whole of the difference between the two clients'
// command sets.
var OPERATIONS = {
    Get: 'GetItem',
    Put: 'PutItem',
    Delete: 'DeleteItem',
    Update: 'UpdateItem',
    Query: 'Query',
    Scan: 'Scan',
    BatchGet: 'BatchGetItem',
    BatchWrite: 'BatchWriteItem',
    TransactGet: 'TransactGetItems',
    TransactWrite: 'TransactWriteItems',
    ExecuteStatement: 'ExecuteStatement',
    ExecuteTransaction: 'ExecuteTransaction'
};

// The members whose value is an ITEM — a map of attribute name to value — wherever they
// appear in a command's input, and the ones whose value is a LIST of items. Every document
// command's input carries its items under one of these names, so the conversion is the one
// walk below rather than a rule per command.
var ITEM_MEMBERS = ['Item', 'Key', 'ExpressionAttributeValues', 'ExclusiveStartKey'];
var ITEM_LIST_MEMBERS = ['Keys', 'Parameters'];

// The same, on the way back out.
var OUTPUT_ITEM_MEMBERS = ['Item', 'Attributes', 'LastEvaluatedKey'];
var OUTPUT_ITEM_LIST_MEMBERS = ['Items'];

function marshalledTree(value, options) {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
        return value;
    }
    var out = Array.isArray(value) ? [] : {};
    var keys = Object.keys(value);
    for (var index = 0; index < keys.length; index += 1) {
        var name = keys[index];
        var held = value[name];
        if (ITEM_MEMBERS.indexOf(name) >= 0) {
            out[name] = conversions.marshall(held, options);
        } else if (ITEM_LIST_MEMBERS.indexOf(name) >= 0 && Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return conversions.marshall(member, options);
            });
        } else if (Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return marshalledTree(member, options);
            });
        } else {
            out[name] = marshalledTree(held, options);
        }
    }
    return out;
}

function unmarshalledTree(value, options) {
    if (value === null || typeof value !== 'object') {
        return value;
    }
    if (Array.isArray(value)) {
        return value.map(function (member) { return unmarshalledTree(member, options); });
    }
    var out = {};
    var keys = Object.keys(value);
    for (var index = 0; index < keys.length; index += 1) {
        var name = keys[index];
        var held = value[name];
        if (OUTPUT_ITEM_MEMBERS.indexOf(name) >= 0) {
            out[name] = conversions.unmarshall(held, options);
        } else if (OUTPUT_ITEM_LIST_MEMBERS.indexOf(name) >= 0 && Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return conversions.unmarshall(member, options);
            });
        } else {
            out[name] = unmarshalledTree(held, options);
        }
    }
    return out;
}

function commandClass(name) {
    var action = name.slice(0, name.length - 'Command'.length);
    var operation = OPERATIONS[action];
    var Command = function (input) {
        if (operation === undefined) {
            throw new TypeError(
                'The document client addresses no operation named ' + action
            );
        }
        this.input = input === undefined || input === null ? {} : input;
        this.operation = operation;
        this.documentCommand = true;
    };
    Object.defineProperty(Command, 'name', { value: name });
    return Command;
}

// "DynamoDBDocumentClient.from(client)": the same client, sending the same calls, with the
// conversion applied on the way in and out.
function DynamoDBDocumentClient(client, translateConfig) {
    this.client = client;
    this.config = client.config;
    var held = translateConfig === undefined || translateConfig === null ? {} : translateConfig;
    this.marshallOptions = held.marshallOptions === undefined ? {} : held.marshallOptions;
    this.unmarshallOptions = held.unmarshallOptions === undefined ? {} : held.unmarshallOptions;
}

DynamoDBDocumentClient.from = function (client, translateConfig) {
    return new DynamoDBDocumentClient(client, translateConfig);
};

DynamoDBDocumentClient.prototype.send = function (command) {
    var self = this;
    if (command === null || command === undefined || command.documentCommand !== true) {
        // A low-level command sent through the document client is sent as it is, which is
        // what the document client does with one.
        return this.client.send(command);
    }
    var translated = {
        input: marshalledTree(command.input, this.marshallOptions),
        operation: command.operation
    };
    return this.client.send(translated).then(function (answered) {
        return unmarshalledTree(answered, self.unmarshallOptions);
    });
};

DynamoDBDocumentClient.prototype.destroy = function () {
    return this.client.destroy();
};

var members = Object.create(null);
members.DynamoDBDocumentClient = DynamoDBDocumentClient;
members.DynamoDBDocument = DynamoDBDocumentClient;

module.exports = new Proxy(members, {
    get: function (target, name) {
        if (typeof name !== 'string' || Object.prototype.hasOwnProperty.call(target, name)) {
            return target[name];
        }
        if (name.length > 'Command'.length
            && name.slice(name.length - 'Command'.length) === 'Command') {
            target[name] = commandClass(name);
            return target[name];
        }
        return undefined;
    }
});
"##;

#[cfg(test)]
mod tests {    use super::*;

    /// A client package of the SDK's own namespace is supplied for any service token, and
    /// nothing else is — one row per form a bare specifier can take.
    #[test]
    fn the_runtime_supplies_a_client_package_for_any_service_token() {
        let supplied: &[&str] = &[
            "runtime:node_modules/@aws-sdk/client-s3",
            "runtime:node_modules/@aws-sdk/client-dynamodb",
            "runtime:node_modules/@aws-sdk/client-lambda",
            "runtime:node_modules/@aws-sdk/client-sqs",
        ];
        for id in supplied {
            let source = runtime_module_source(id);
            assert!(
                source.is_some_and(|text| text.contains("Client.prototype.send")),
                "case {id:?}: expected the client package's source, got none"
            );
        }
        let absent: &[&str] = &[
            "runtime:node_modules/@aws-sdk/client-",
            "runtime:node_modules/@aws-sdk/lib-s3",
            "runtime:node_modules/express",
            "runtime:node_modules/@aws-sdk/client-s3/../../etc/passwd",
            "node_modules/@aws-sdk/client-s3",
            "layer:node_modules/@aws-sdk/client-s3",
        ];
        for id in absent {
            assert!(
                runtime_module_source(id).is_none(),
                "case {id:?}: this runtime supplies no such module, so none must resolve"
            );
        }
        // The two packages that are not a service client: the document client and the
        // value conversions it is built on.
        for id in [
            "runtime:node_modules/@aws-sdk/lib-dynamodb",
            "runtime:node_modules/@aws-sdk/util-dynamodb",
        ] {
            assert!(
                runtime_module_source(id).is_some(),
                "case {id:?}: expected the package's source, got none"
            );
        }
    }

    /// The token the package name carries is the one baked into the module, so the call a
    /// client makes names the service its own package does, and the client is exported
    /// under the name that package exports it by.
    #[test]
    fn the_client_names_the_service_its_package_does() {
        let cases: &[(&str, &str)] = &[
            ("s3", "S3Client"),
            ("dynamodb", "DynamoDBClient"),
            ("sqs", "SQSClient"),
            ("lambda", "LambdaClient"),
            ("eventbridge", "EventBridgeClient"),
            ("secrets-manager", "SecretsManagerClient"),
            ("scheduler", "SchedulerClient"),
        ];
        for (token, exported) in cases {
            let id = format!("runtime:node_modules/@aws-sdk/client-{token}");
            let source = runtime_module_source(&id).expect("the client package's source");
            assert!(
                source.contains(&format!("var SERVICE = '{token}';")),
                "case {token:?}: expected the package's own token in the module, got a module \
                 naming another service"
            );
            assert!(
                source.contains(&format!("members.{exported} = Client;")),
                "case {token:?}: expected the client exported as {exported}, got a module \
                 exporting another name"
            );
        }
    }
}
