Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront.op-contract-deletePublicKey
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront.op-contract-deletePublicKey|_error#25|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# cloudfront: CreateKeyGroup accepts a KeyGroupConfig.Items entry naming a public key that does not exist, returning 201 instead of InvalidArgument

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKeyGroup

## Description
This issue carries ONE root cause in the cloudfront service; it must be closed before submitting.

Cause 1 of 1: CreateKeyGroup never resolves the public-key ids in KeyGroupConfig.Items against public-key state, so a group that references a nonexistent (e.g. just-deleted) public key is created instead of refused.

AWS: Smithy: CreateKeyGroup -> POST /2020-05-31/key-group, success HTTP 201 with the KeyGroup payload plus Location and ETag headers; modeled errors InvalidArgument (400), KeyGroupAlreadyExists (409), TooManyKeyGroups (400), TooManyPublicKeysInKeyGroup (400). Every entry of KeyGroupConfig.Items must name a CloudFront public key that exists in the account; CloudFront treats an entry that resolves to nothing as a malformed request and refuses the create with `InvalidArgument` (HTTP 400) and the message "The specified CloudFront public key does not exist in key group." — that text is entirely fixed, it carries no value from the request or from state — and no key group is created. Correspondingly GetPublicKey for that id answers NoSuchPublicKey (404) "The specified CloudFront public key does not exist.", which is the state the refusal is derived from.
Emulator: the create SUCCEEDS. For a KeyGroupConfig whose single Items entry is a public-key id that was deleted moments before (GetPublicKey on it already answers NoSuchPublicKey), the emulator answers HTTP 201 with a Location header "https://cloudfront.amazonaws.com/2020-05-31/key-group/<new id>", an ETag, and a KeyGroup body whose KeyGroupConfig.Items echoes the dangling key id back. Reading services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_key_group.rs confirms the handler validates only emptiness of Items, comment length, the Items count bound, duplicate Name and the key-group count bound, and then stores the config whole — nothing resolves the ids it carries.
Why: a key group is an authorization boundary — a cache behavior that lists it only serves requests signed by one of its keys. A client that mistypes or reuses a retired key id believes it has configured a signer set, and the group it gets back admits no signature at all, so every signed-URL/signed-cookie request to a distribution trusting that group fails later at the edge with no indication of the real cause. Referential integrity is also asymmetric in the emulator today: DeletePublicKey does refuse with PublicKeyInUse while a group lists the key, but CreateKeyGroup will happily (re)introduce a reference to a key that is already gone, so the store can hold a dangling reference that AWS makes impossible. A client written against AWS also loses the 400-vs-201 distinction it uses to fail fast in its own deploy pipeline.
Fix: CreateKeyGroup's input validation in services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/ops/create_key_group.rs — after the shape checks and before the id/ETag are minted and the record inserted, resolve every KeyGroupConfig.Items entry against the crate's public-key state and refuse an unresolvable entry with the operation's modeled InvalidArgument carrying AWS's fixed sentence. UpdateKeyGroup must make the same judgement on the items it writes, so the check belongs in the shared key-group-config validation path rather than only in this handler. Related but distinct open issue: 20261027T001538Z-cloudfront-nine-control-plane-divergences-deletepublickey-updatekeygroup-deletefieldlevelencryptionprofile.md (its Cause 8 is about CreateKeyGroup's duplicate-name MESSAGE text, not about resolving Items) — it does not carry this check or this rule.
Traces: cloudfront.op-contract-deletePublicKey.

## Evidence
Cause 1 — cloudfront.op-contract-deletePublicKey [mismatch]:
  AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 201, 'HTTPHeaders': {'content-type': 'application/xml', 'location': 'https://cloudfront.amazonaws.com/2020-05-31/key-group/<id>', 'etag': '<etag>', ...}}, 'Location': 'https://cloudfront.amazonaws.com/2020-05-31/key-group/<id>', 'ETag': '<etag>', 'KeyGroup': {'Id': '<id>', 'LastModifiedTime': <timestamp>, 'KeyGroupConfig': {'Name': '<name>-kg2', 'Items': ['<the deleted public key id>'], 'Comment': 'should fail: references a deleted public key'}}}

  The sequence that reached this point all behaved correctly: CreatePublicKey, CreateKeyGroup naming the key, DeletePublicKey refused with PublicKeyInUse, DeleteKeyGroup, DeletePublicKey succeeded, and GetPublicKey on the deleted id answered NoSuchPublicKey "The specified CloudFront public key does not exist.". Only the final leg diverges.

  Request: CreateKeyGroup(KeyGroupConfig={Name: <a fresh unique name>, Items: [<the id of the public key just deleted>], Comment: ...}).
  Expected: HTTP 400, code InvalidArgument, message "The specified CloudFront public key does not exist in key group." (fixed text, no variable part), and no key group created.
  Actual: HTTP 201 Created with a Location header, an ETag and a KeyGroup body echoing the dangling key id.
  The key-group id, key id, ETag, generated name and LastModifiedTime above are per-run values and are not part of the requirement; the 201-instead-of-400 and the stored dangling reference are.

## Steps to Reproduce
1. cloudfront CreatePublicKey with a freshly generated RSA SubjectPublicKeyInfo PEM; note PublicKey.Id and the ETag.
2. cloudfront CreateKeyGroup(KeyGroupConfig={Name: <unique>, Items: [<that key id>]}); note KeyGroup.Id and its ETag.
3. DeleteKeyGroup(Id=<key group id>, IfMatch=<its ETag>), then DeletePublicKey(Id=<key id>, IfMatch=<current key ETag>) — both succeed.
4. GetPublicKey(Id=<key id>) -> HTTP 404 NoSuchPublicKey "The specified CloudFront public key does not exist." (confirms the key is gone).
5. CreateKeyGroup(KeyGroupConfig={Name: <a second unique name>, Items: [<the same, now deleted, key id>], Comment: "..."}) -> expect HTTP 400 InvalidArgument with "The specified CloudFront public key does not exist in key group." and no key group created; observe HTTP 201 with a Location header, an ETag and a KeyGroup whose Items echoes the deleted id. ListKeyGroups/GetKeyGroup then show a stored group holding a reference AWS makes unrepresentable.
6. Re-replay the check id in trace_ids to confirm the fix.
