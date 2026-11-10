Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable
Issue-Key: ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable|run#9|AssertionError|assert code == "InvalidParameterException", (
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# ds UpdateNumberOfDomainControllers on a Creating directory answers DirectoryUnavailableException instead of InvalidParameterException

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateNumberOfDomainControllers

## Description
One net-new root cause. Cause 1 of 1: the wrong modeled error class, with emulator-authored message text, for a directory that is not yet Active.

AWS: Smithy com.amazonaws.directoryservice#UpdateNumberOfDomainControllers (input DirectoryId required, pattern ^d-[0-9a-f]{10}$; DesiredNumber required, range min 2) models ClientException, DirectoryUnavailableException, DomainControllerLimitExceededException, EntityDoesNotExistException, InvalidParameterException, ServiceException and UnsupportedOperationException. Scaling a directory whose Stage is still Creating is refused with HTTP 400 InvalidParameterException and the message "The directory is currently unavailable for updates. The directory status must be Active when making changes. Please try again in a few minutes." — that sentence is fixed text, with no value interpolated from the request or state (the directory id does not appear in it). The refusal must leave DesiredNumberOfDomainControllers and Stage unchanged.

Emulator: HTTP 400 DirectoryUnavailableException with the message "Operation could not be performed because directory <directory-id> is unavailable." — a different modeled error class, and emulator-authored prose that interpolates the directory id where AWS's message has no variable part.

Why: a client branches on the error code. InvalidParameterException is a caller-state error that tells the client to wait for Stage == Active and retry; DirectoryUnavailableException is the class AWS reserves for a directory that has become unavailable after creation, which callers treat as an operational fault (alarm/abort) rather than "not ready yet". The wrong class turns a normal provisioning race into a false failure, and the differing message defeats any text-matching in client tooling.

Fix: the pre-condition validation of the operation handler for UpdateNumberOfDomainControllers in services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/ — classify a non-Active (Creating) directory as InvalidParameterException with AWS's fixed message, and reserve DirectoryUnavailableException for the states AWS uses it for.

Traces: ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable.

## Evidence
Replay output for ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable:
AssertionError: scaling a Creating directory should be refused InvalidParameterException, got 'DirectoryUnavailableException': Operation could not be performed because directory d-003ba5fe5a is unavailable.

Expected code: InvalidParameterException. Expected message (fixed text, no variable parts): "The directory is currently unavailable for updates. The directory status must be Active when making changes. Please try again in a few minutes." The directory id in the actual message is a per-run value and is not part of AWS's template.

## Steps to Reproduce
1. ds:CreateMicrosoftAD into a VPC with two subnets and immediately read DescribeDirectories until Stage == Creating.
2. ds:UpdateNumberOfDomainControllers(DirectoryId=<id>, DesiredNumber=3).
3. Expected 400 InvalidParameterException with "The directory is currently unavailable for updates. The directory status must be Active when making changes. Please try again in a few minutes."; the emulator answers DirectoryUnavailableException with its own prose.
4. Re-replay ds__ec2.update-number-of-domain-controllers-on-a-creating-directory-rejected-directory-unavailable.
