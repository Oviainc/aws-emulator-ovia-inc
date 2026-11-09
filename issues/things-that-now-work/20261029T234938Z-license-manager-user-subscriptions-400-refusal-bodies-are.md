Issue-Source: trace-sweep-divergence
Issue-Trace: license-manager-user-subscriptions-list-instances-bogus-filter-attribute-rejected
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: license-manager-user-subscriptions-list-instances-bogus-filter-attribute-rejected license-manager-user-subscriptions-deregister-identity-provider-not-found license-manager-user-subscriptions-register-identity-provider-unknown-directory
Issue-Shape-Id: com.amazonaws.licensemanagerusersubscriptions#LicenseManagerUserSubscriptions
Issue-Key: license-manager-user-subscriptions|ListInstances|4xx->4xx|$.message
Issue-Key: license-manager-user-subscriptions|RegisterIdentityProvider|4xx->4xx|$.message
Issue-Related: 20261029T234938Z-license-manager-user-subscriptions-unauthorized-refusals-answer-http.md
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# license-manager-user-subscriptions 400 refusal bodies are emulator-authored and ordered wrongly

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
SHARED FAULT (3 causes): on the paths where the emulator does pick the right status (HTTP 400) and the right error class, the BODY still diverges - the message is written by the emulator rather than reproduced as the service words it, and in one case the wrong precondition is reported because the checks run in the wrong order.

Cause 1 of 3 - ListInstances with a Filter.Attribute the service does not support.
AWS: recorded behavior only (Smithy models Filter as a free-form structure of Attribute/Operation/Value Strings with no enum, so the set of filterable attributes is not in the model; the operation's refusal class is ValidationException, httpError 400 defaulted). The recording shows HTTP 400 with message "Invalid Filter attribute NotARealAttributeXYZ". The requirement is that template: the fixed text "Invalid Filter attribute " followed by ONE variable part, the rejected attribute name copied from the request's Filter.Attribute (NotARealAttributeXYZ is this recording's value, not part of the template).
Emulator: HTTP 400 with the self-authored message "Filter.Attribute NotARealAttributeXYZ is not filterable; supported attributes: Status, InstanceId" - different fixed text, a member-path prefix the service does not use, and an invented enumeration of supported attributes appended.
Why: a client or test that matches the service's refusal text does not match, and the appended "supported attributes" list is emulator-invented guidance that does not reflect what the service actually accepts.
Fix: the ListInstances filter validation in that operation's handler - emit the service's message template instead of composing one locally.
Traces: license-manager-user-subscriptions-list-instances-bogus-filter-attribute-rejected#task#1.

Cause 2 of 3 - the constraint-violation message quotes the regular expression WITH its anchors.
AWS: Smithy: DeregisterIdentityProviderRequest.IdentityProviderArn targets Arn with @pattern ^arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}$; the violation is ValidationException (httpError 400). The recorded refusal body is the standard constraint sentence "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where the pattern is rendered WITHOUT the leading ^ and trailing $ anchors, and the value and member name come from the request.
Emulator: identical status, class, fixed text, value and member name, but it renders the pattern with the anchors included ("...pattern: ^arn:...$").
Why: the message is a requirement of the error class; a client that compares or parses the quoted pattern - or a fixture that asserts the refusal body - sees text the service never emits. This is a general rendering rule, not one member: every pattern-constraint message this service emits must strip the anchors.
Fix: the shared constraint-violation message renderer used by this crate's input validation (strip the ^/$ anchors when interpolating the pattern), not the DeregisterIdentityProvider handler alone.
Traces: license-manager-user-subscriptions-deregister-identity-provider-not-found#task#3.

Cause 3 of 3 - RegisterIdentityProvider reports the security group before the directory.
AWS: recorded behavior only - for a registration naming a directory that does not exist (or is not active) together with Settings naming network resources, the service refuses with HTTP 400 and the message template "<directoryId> directory either cannot be found or is not in active state", whose only variable part is the DirectoryId copied from the request's IdentityProvider (d-2eabeceeaa is this recording's value). The directory prerequisite is evaluated before the Settings' security group.
Emulator: HTTP 400 but with "The security group '<sg-id>' does not exist" - it resolves Settings.SecurityGroupId first and never reports the directory failure, so the caller is told about the wrong missing prerequisite.
Why: a client fixing the reported problem (creating/correcting the security group) retries and is refused again, because the actual blocker - the directory - was never named. The unreachable directory message also means no client can discover that the directory id was wrong.
Fix: the RegisterIdentityProvider handler's precondition ordering - resolve and refuse on the identity provider's directory before validating Settings' subnets/security group, and use the service's directory-refusal template.
Traces: license-manager-user-subscriptions-register-identity-provider-unknown-directory#task#3.

All three causes must be closed. Open issues 20261024T010614Z-license-manager-user-subscriptions-registeridentityprovider-wrong-behavior.md and 20261024T124828Z-license-manager-user-subscriptions-registeridentityprovider-event-propagation.md touch the same handler but are about how directories and network resources are resolved, not about which prerequisite is reported first or the refusal wording, and neither carries these replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 62.2%, 14 mismatched steps of 37). Per cause:
1. license-manager-user-subscriptions-list-instances-bogus-filter-attribute-rejected#task#1 ListInstances - status 400 matched; $.message expected 'Invalid Filter attribute NotARealAttributeXYZ', got 'Filter.Attribute NotARealAttributeXYZ is not filterable; supported attributes: Status, InstanceId'.
2. license-manager-user-subscriptions-deregister-identity-provider-not-found#task#3 DeregisterIdentityProvider - status 400 matched; $.message expected "1 validation error detected: Value 'arn:aws:license-manager-user-subscriptions:us-west-2:000000000000:identity-provider/d-2eabeceeaa/VISUAL_STUDIO_PROFESSIONAL' at 'identityProviderArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-z0-9-\.]{1,63}:[a-zA-Z0-9-\.]{1,510}/[a-zA-Z0-9-\.]{1,510}", got the same sentence with the pattern rendered as ^arn:...$ (leading ^ and trailing $ present). The only difference is the two anchor characters.
3. license-manager-user-subscriptions-register-identity-provider-unknown-directory#task#3 RegisterIdentityProvider - status 400 matched; $.message expected 'd-2eabeceeaa directory either cannot be found or is not in active state', got "The security group 'sg-sim2real' does not exist".
Model evidence: Filter.Attribute/Operation/Value are plain Strings (no enum of filterable attributes); IdentityProviderArn/InstanceUserArn carry the anchored Arn @pattern; ValidationException is errorKind client with httpError 400 (defaulted).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Representative: fixture license-manager-user-subscriptions-list-instances-bogus-filter-attribute-rejected, step task#1 - POST /instance/ListInstances with Filter.Attribute set to an unsupported attribute name. Status 400 is right; compare $.message against the recorded 'Invalid Filter attribute <attribute>'.
4. Cause 2: fixture license-manager-user-subscriptions-deregister-identity-provider-not-found, step task#3 - POST /identity-provider/DeregisterIdentityProvider with an IdentityProviderArn that violates the Arn pattern; the refusal body quotes the pattern with ^ and $, which the recorded body omits.
5. Cause 3: fixture license-manager-user-subscriptions-register-identity-provider-unknown-directory, step task#3 - POST /identity-provider/RegisterIdentityProvider for a non-existent directory with Settings naming a non-existent security group; the emulator reports the security group, AWS reports the directory.
