#!/usr/bin/env node

import { createHash, createPublicKey, verify as verifySignature } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const PROFILE = 'coven.automations.authority.v1';
const BASE_PROFILE = 'coven.automations.v1';
const RUNTIME_AUTHORITY_CAPABILITY = 'automations.runtime-authority.v1';
const AUTHORITY_EXTENSION_KEY = PROFILE;
const BINDING_DOMAIN = 'opencoven:coven-automations-authority-binding:v1';
const RECEIPT_DOMAIN = 'opencoven:coven-automations-authority-receipt-evidence:v1';
const PROFILE_DIR = path.join(
  path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..'),
  'spec',
  'coven-automations',
  'authority',
  'v1'
);

const BINDING_KEYS = new Set([
  'profile',
  'kind',
  'bindingId',
  'base',
  'principal',
  'authorization',
  'familiar',
  'contextProjection',
  'threads',
  'capabilities',
  'approval',
  'risk',
  'runtime',
  'versions',
  'decisionTimestamp',
  'producer',
  'privacy',
  'integrity',
  'authentication'
]);

const RECEIPT_KEYS = new Set([
  'profile',
  'kind',
  'receiptId',
  'automationId',
  'automationRevision',
  'definitionDigest',
  'occurrenceId',
  'occurrenceFenceGeneration',
  'runId',
  'attemptId',
  'attemptNumber',
  'baseReceiptDigest',
  'bindingId',
  'bindingDigest',
  'principalId',
  'familiar',
  'authorization',
  'capabilities',
  'approval',
  'risk',
  'runtime',
  'decisionTimestamp',
  'producer',
  'privacy',
  'integrity',
  'authentication'
]);

const EXTENSION_KEYS = new Set([
  'profile',
  'kind',
  'executionBinding',
  'receiptEvidence'
]);

const DISPATCH_CONSUMPTION_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: [
    'bindingId',
    'nonce',
    'adoptionKey',
    'occurrenceId',
    'runId',
    'attemptId',
    'attemptNumber',
    'fenceGeneration',
    'approval'
  ],
  properties: {
    bindingId: { $ref: 'common.schema.json#/$defs/opaqueIdentifier' },
    nonce: { $ref: 'common.schema.json#/$defs/opaqueIdentifier' },
    adoptionKey: { $ref: 'common.schema.json#/$defs/baseAdoptionKey' },
    occurrenceId: { $ref: 'common.schema.json#/$defs/baseOccurrenceId' },
    runId: { $ref: 'common.schema.json#/$defs/baseRunId' },
    attemptId: { $ref: 'common.schema.json#/$defs/baseAttemptId' },
    attemptNumber: { type: 'integer', minimum: 1, maximum: 9007199254740991 },
    fenceGeneration: { type: 'integer', minimum: 1, maximum: 9007199254740991 },
    approval: {}
  }
};

const DISPATCH_APPROVAL_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['requirement', 'approvalId', 'use', 'consumption'],
  properties: {
    requirement: {
      enum: ['human_per_run', 'protected_owner_per_run', 'bounded_recurring']
    },
    approvalId: { $ref: 'common.schema.json#/$defs/opaqueIdentifier' },
    use: {},
    consumption: {}
  }
};

const TRUSTED_PLACEHOLDER_DIGEST = {
  algorithm: 'sha256',
  canonicalization: 'jcs-rfc8785',
  value: '0'.repeat(64)
};

const TRUSTED_REQUIRED_KEYS = [
  'dispatchNow',
  'receiptId',
  'bindingId',
  'bindingDigest',
  'baseReceiptDigest',
  'automationId',
  'automationRevision',
  'principalId',
  'principalAuthorizationProofRef',
  'familiarRootId',
  'identityRevisionId',
  'familiarDeclarationDigest',
  'familiarEmbodimentBindingId',
  'familiarEmbodimentDigest',
  'familiarValidFrom',
  'familiarValidUntil',
  'familiarRevocationState',
  'familiarRevocationCheckedAt',
  'familiarRetirementState',
  'familiarRetirementCheckedAt',
  'definitionDigest',
  'occurrenceId',
  'occurrenceKey',
  'occurrenceFenceGeneration',
  'runId',
  'attemptId',
  'attemptNumber',
  'adoptionKey',
  'projectId',
  'workspaceId',
  'contextProjectionIds',
  'memoryProjectionIds',
  'threadsDecisionDigest',
  'protectedSurfaceManifestId',
  'protectedSurfaceManifestDigest',
  'runtimeDescriptorDigest',
  'runtimeId',
  'runtimeDescriptorVersion',
  'runtimeSelectionRationale',
  'runtimeCapabilities',
  'policyDigest',
  'policyVersion',
  'decisionTimestamp',
  'familiarFreshnessPolicyVersion',
  'familiarFreshnessBoundSeconds',
  'authorizationOperation',
  'authorizationRequestId',
  'authorizationRequestDigest',
  'authorizationOutcome',
  'consumptionSnapshotDigest',
  'requestedCapabilities',
  'grantedCapabilities',
  'deniedCapabilities',
  'degradedCapabilities',
  'approvalRequirement',
  'approvalId',
  'approvalDigest',
  'approvalScopeDigest',
  'approvalExpiresAt',
  'approvalConsumptionDigest',
  'approvalUse',
  'approvalConsumption',
  'riskClass',
  'sideEffectClass',
  'privacyClassification',
  'privacyRetention',
  'privacyRedactionStatus',
  'authenticationProofs',
  'replayedNonces',
  'replayedAdoptionKeys',
  'consumedApprovalIds',
  'consumedRecurringOccurrences',
  'dispatchConsumptions'
];

export class AuthorityProfileError extends Error {
  constructor(code, message) {
    super(message);
    this.name = 'AuthorityProfileError';
    this.code = code;
  }
}

function trustedSchemaErrors(value, schema) {
  const schemas = schemaRegistry();
  return schemaErrors(value, schema, schema, schemas);
}

function refuseTrustedDispatch(index, detail) {
  refuse(
    'AUTHORITY_TRUSTED_STATE_UNAVAILABLE',
    `Trusted dispatch consumption ${index} is invalid: ${detail}`
  );
}

function assertTrustedDispatchConsumption(entry, index) {
  const recordErrors = trustedSchemaErrors(entry, DISPATCH_CONSUMPTION_SCHEMA);
  if (recordErrors.length > 0) {
    refuseTrustedDispatch(index, recordErrors[0]);
  }
  if (entry.approval === null) {
    return;
  }
  const approvalErrors = trustedSchemaErrors(entry.approval, DISPATCH_APPROVAL_SCHEMA);
  if (approvalErrors.length > 0) {
    refuseTrustedDispatch(index, approvalErrors[0]);
  }

  const syntheticApproval = {
    requirement: entry.approval.requirement,
    evidence: {
      approvalId: entry.approval.approvalId,
      approvalDigest: TRUSTED_PLACEHOLDER_DIGEST,
      state: 'approved'
    },
    scopeDigest: TRUSTED_PLACEHOLDER_DIGEST,
    expiresAt: '2000-01-01T00:00:00.000Z',
    use: entry.approval.use,
    consumption: entry.approval.consumption
  };
  const schemas = schemaRegistry();
  const common = schemas.get('common.schema.json');
  const shapeErrors = schemaErrors(
    syntheticApproval,
    common.$defs.approvalBinding,
    common,
    schemas
  );
  if (shapeErrors.length > 0) {
    refuseTrustedDispatch(index, shapeErrors[0]);
  }

  const consumption = entry.approval.consumption;
  if (
    consumption.occurrenceId !== entry.occurrenceId ||
    consumption.runId !== entry.runId ||
    consumption.attemptNumber !== entry.attemptNumber ||
    consumption.fenceGeneration !== entry.fenceGeneration
  ) {
    refuseTrustedDispatch(index, 'approval consumption does not match its ownership record');
  }
  if (entry.approval.requirement === 'bounded_recurring') {
    const use = entry.approval.use;
    if (
      !entry.occurrenceId.startsWith(use.occurrencePrefix) ||
      use.priorUses >= use.maxUses ||
      consumption.usageNumber !== use.priorUses + 1
    ) {
      refuseTrustedDispatch(index, 'bounded recurring approval semantics are invalid');
    }
  }
}

function refuse(code, message) {
  throw new AuthorityProfileError(code, message);
}

function isPlainObject(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function assertWellFormedString(value) {
  for (let index = 0; index < value.length; index += 1) {
    const codeUnit = value.charCodeAt(index);
    if (codeUnit >= 0xd800 && codeUnit <= 0xdbff) {
      const next = value.charCodeAt(index + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) {
        refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON contains an unpaired high surrogate');
      }
      index += 1;
    } else if (codeUnit >= 0xdc00 && codeUnit <= 0xdfff) {
      refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON contains an unpaired low surrogate');
    }
  }
}

function assertIJson(value, ancestors = new Set()) {
  if (typeof value === 'string') {
    assertWellFormedString(value);
    return;
  }
  if (value === null || typeof value === 'boolean') {
    return;
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value)) {
      refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON contains a non-finite number');
    }
    return;
  }
  if (typeof value !== 'object') {
    refuse('AUTHORITY_IJSON_INVALID', 'Authority value is not JSON');
  }
  if (ancestors.has(value)) {
    refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON contains a cycle');
  }
  ancestors.add(value);
  if (Array.isArray(value)) {
    const ownKeys = Reflect.ownKeys(value);
    if (
      ownKeys.some(
        (key) =>
          typeof key !== 'string' ||
          (key !== 'length' &&
            (!/^(0|[1-9][0-9]*)$/.test(key) ||
              Number(key) >= value.length ||
              String(Number(key)) !== key))
      )
    ) {
      refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON array has non-index properties');
    }
    for (let index = 0; index < value.length; index += 1) {
      if (!Object.hasOwn(value, index)) {
        refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON array is sparse');
      }
      const descriptor = Object.getOwnPropertyDescriptor(value, String(index));
      if (!descriptor?.enumerable || !Object.hasOwn(descriptor, 'value')) {
        refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON array has non-data entries');
      }
      assertIJson(value[index], ancestors);
    }
    ancestors.delete(value);
    return;
  }
  if (isPlainObject(value)) {
    for (const key of Reflect.ownKeys(value)) {
      if (typeof key !== 'string') {
        refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON object has a symbol key');
      }
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (!descriptor?.enumerable || !Object.hasOwn(descriptor, 'value')) {
        refuse('AUTHORITY_IJSON_INVALID', 'Authority JSON object has a non-data property');
      }
      assertWellFormedString(key);
      assertIJson(descriptor.value, ancestors);
    }
    ancestors.delete(value);
    return;
  }
  refuse('AUTHORITY_IJSON_INVALID', 'Authority value contains a non-JSON object');
}

function canonicalJson(value) {
  if (value === null || typeof value === 'boolean' || typeof value === 'string') {
    if (typeof value === 'string') {
      assertWellFormedString(value);
    }
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) {
      refuse('AUTHORITY_SCHEMA_INVALID', 'Authority profile numbers must be safe integers');
    }
    return JSON.stringify(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(',')}]`;
  }
  if (!isPlainObject(value)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority profile values must be JSON values');
  }
  return `{${Object.keys(value)
    .sort()
    .map((key) => {
      assertWellFormedString(key);
      return `${JSON.stringify(key)}:${canonicalJson(value[key])}`;
    })
    .join(',')}}`;
}

function isTimestamp(value) {
  if (
    typeof value !== 'string' ||
    !/^[0-9]{4}-(0[1-9]|1[0-2])-([0-2][0-9]|3[01])T([01][0-9]|2[0-3]):[0-5][0-9]:[0-5][0-9](\.[0-9]{3})?Z$/.test(
      value
    )
  ) {
    return false;
  }
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) {
    return false;
  }
  const normalized = new Date(milliseconds).toISOString();
  return value.includes('.') ? normalized === value : normalized.replace('.000Z', 'Z') === value;
}

function schemaRegistry() {
  return new Map(
    [
      'common.schema.json',
      'authority-extension.schema.json',
      'automation-execution-binding.schema.json',
      'automation-receipt-authority-evidence.schema.json'
    ].map((name) => [name, JSON.parse(readFileSync(path.join(PROFILE_DIR, name), 'utf8'))])
  );
}

function resolvePointer(value, fragment) {
  if (!fragment) {
    return value;
  }
  return fragment
    .replace(/^#\//, '')
    .split('/')
    .map((component) => component.replaceAll('~1', '/').replaceAll('~0', '~'))
    .reduce((current, component) => current?.[component], value);
}

function schemaErrors(value, schema, rootSchema, schemas, location = '$') {
  if (schema.$ref) {
    const [fileName, fragment = ''] = schema.$ref.split('#');
    const referencedRoot = fileName ? schemas.get(fileName) : rootSchema;
    const referenced = resolvePointer(referencedRoot, fragment ? `#${fragment}` : '');
    if (!referenced) {
      return [`${location}: unresolved schema reference ${schema.$ref}`];
    }
    return schemaErrors(value, referenced, referencedRoot, schemas, location);
  }

  if (schema.oneOf) {
    const matching = schema.oneOf.filter(
      (candidate) => schemaErrors(value, candidate, rootSchema, schemas, location).length === 0
    );
    return matching.length === 1 ? [] : [`${location}: expected exactly one schema branch`];
  }
  if (schema.allOf) {
    const errors = schema.allOf.flatMap((candidate) =>
      schemaErrors(value, candidate, rootSchema, schemas, location)
    );
    if (errors.length > 0) {
      return errors;
    }
  }
  if (schema.if) {
    const conditionMatches = schemaErrors(value, schema.if, rootSchema, schemas, location).length === 0;
    const branch = conditionMatches ? schema.then : schema.else;
    if (branch) {
      const errors = schemaErrors(value, branch, rootSchema, schemas, location);
      if (errors.length > 0) {
        return errors;
      }
    }
  }
  if (schema.type === undefined && (schema.required || schema.properties)) {
    if (!isPlainObject(value)) {
      return [];
    }
    const errors = [];
    for (const required of schema.required ?? []) {
      if (!Object.hasOwn(value, required)) {
        errors.push(`${location}: missing ${required}`);
      }
    }
    for (const [key, childSchema] of Object.entries(schema.properties ?? {})) {
      if (Object.hasOwn(value, key)) {
        errors.push(
          ...schemaErrors(value[key], childSchema, rootSchema, schemas, `${location}/${key}`)
        );
      }
    }
    if (errors.length > 0) {
      return errors;
    }
  }

  if (Object.hasOwn(schema, 'const') && canonicalJson(value) !== canonicalJson(schema.const)) {
    return [`${location}: value does not match const`];
  }
  if (
    schema.enum &&
    !schema.enum.some((candidate) => canonicalJson(value) === canonicalJson(candidate))
  ) {
    return [`${location}: value is not in enum`];
  }

  if (schema.type === 'null') {
    return value === null ? [] : [`${location}: expected null`];
  }
  if (schema.type === 'boolean') {
    return typeof value === 'boolean' ? [] : [`${location}: expected boolean`];
  }
  if (schema.type === 'integer') {
    if (!Number.isSafeInteger(value)) {
      return [`${location}: expected safe integer`];
    }
    if (schema.minimum !== undefined && value < schema.minimum) {
      return [`${location}: integer below minimum`];
    }
    if (schema.maximum !== undefined && value > schema.maximum) {
      return [`${location}: integer above maximum`];
    }
    return [];
  }
  if (schema.type === 'string') {
    if (typeof value !== 'string') {
      return [`${location}: expected string`];
    }
    const length = [...value].length;
    if (schema.minLength !== undefined && length < schema.minLength) {
      return [`${location}: string below minimum length`];
    }
    if (schema.maxLength !== undefined && length > schema.maxLength) {
      return [`${location}: string above maximum length`];
    }
    if (schema.pattern && !new RegExp(schema.pattern).test(value)) {
      return [`${location}: string does not match pattern`];
    }
    if (schema.format === 'date-time' && !isTimestamp(value)) {
      return [`${location}: invalid date-time`];
    }
    return [];
  }
  if (schema.type === 'array') {
    if (!Array.isArray(value)) {
      return [`${location}: expected array`];
    }
    const errors = [];
    if (schema.minItems !== undefined && value.length < schema.minItems) {
      errors.push(`${location}: array below minimum length`);
    }
    if (schema.maxItems !== undefined && value.length > schema.maxItems) {
      errors.push(`${location}: array above maximum length`);
    }
    if (schema.uniqueItems) {
      const serialized = value.map(canonicalJson);
      if (new Set(serialized).size !== serialized.length) {
        errors.push(`${location}: array items are not unique`);
      }
    }
    if (schema.items) {
      value.forEach((item, index) => {
        errors.push(...schemaErrors(item, schema.items, rootSchema, schemas, `${location}/${index}`));
      });
    }
    return errors;
  }
  if (schema.type === 'object') {
    if (!isPlainObject(value)) {
      return [`${location}: expected object`];
    }
    const errors = [];
    for (const required of schema.required ?? []) {
      if (!Object.hasOwn(value, required)) {
        errors.push(`${location}: missing ${required}`);
      }
    }
    if (schema.additionalProperties === false) {
      for (const key of Object.keys(value)) {
        if (!Object.hasOwn(schema.properties ?? {}, key)) {
          errors.push(`${location}: unknown field ${key}`);
        }
      }
    }
    for (const [key, childSchema] of Object.entries(schema.properties ?? {})) {
      if (Object.hasOwn(value, key)) {
        errors.push(
          ...schemaErrors(value[key], childSchema, rootSchema, schemas, `${location}/${key}`)
        );
      }
    }
    return errors;
  }
  return [];
}

function assertSchema(value, schemaName) {
  const schemas = schemaRegistry();
  const schema = schemas.get(schemaName);
  const errors = schemaErrors(value, schema, schema, schemas);
  if (errors.length > 0) {
    refuse('AUTHORITY_SCHEMA_INVALID', errors[0]);
  }
}

export function computeAuthorityDigest(value, target) {
  const unsigned = structuredClone(value);
  delete unsigned.integrity;
  delete unsigned.authentication;
  const domain = target === 'binding' ? BINDING_DOMAIN : RECEIPT_DOMAIN;
  return createHash('sha256')
    .update(Buffer.from(domain))
    .update(Buffer.from([0]))
    .update(Buffer.from(canonicalJson(unsigned)))
    .digest('hex');
}

function assertProfile(value) {
  if (!isPlainObject(value)) {
    refuse('AUTHORITY_PROFILE_MISSING', 'Authority companion value is absent');
  }
  if (!Object.hasOwn(value, 'profile')) {
    refuse('AUTHORITY_PROFILE_MISSING', 'Authority companion profile is missing');
  }
  if (typeof value.profile !== 'string') {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority companion profile must be a string');
  }
  if (value.profile !== PROFILE) {
    refuse('AUTHORITY_PROFILE_UNKNOWN', `Unsupported authority profile ${value.profile}`);
  }
}

function assertTrustedState(trusted) {
  if (!isPlainObject(trusted)) {
    refuse(
      'AUTHORITY_TRUSTED_STATE_UNAVAILABLE',
      'Trusted authority state is unavailable'
    );
  }
  const missing = TRUSTED_REQUIRED_KEYS.find((key) => !Object.hasOwn(trusted, key));
  if (missing) {
    refuse(
      'AUTHORITY_TRUSTED_STATE_UNAVAILABLE',
      `Trusted authority state is missing ${missing}`
    );
  }
  for (const key of [
    'contextProjectionIds',
    'memoryProjectionIds',
    'runtimeCapabilities',
    'requestedCapabilities',
    'grantedCapabilities',
    'deniedCapabilities',
    'degradedCapabilities',
    'replayedNonces',
    'replayedAdoptionKeys',
    'consumedApprovalIds',
    'consumedRecurringOccurrences',
    'dispatchConsumptions'
  ]) {
    if (!Array.isArray(trusted[key])) {
      refuse(
        'AUTHORITY_TRUSTED_STATE_UNAVAILABLE',
        `Trusted authority state ${key} must be an array`
      );
    }
  }
  for (const key of ['approvalUse', 'approvalConsumption', 'authenticationProofs']) {
    if (!isPlainObject(trusted[key])) {
      refuse(
        'AUTHORITY_TRUSTED_STATE_UNAVAILABLE',
        `Trusted authority state ${key} must be an object`
      );
    }
  }
  trusted.dispatchConsumptions.forEach(assertTrustedDispatchConsumption);
}

function assertClosedTopLevel(value, allowedKeys) {
  if (!isPlainObject(value)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority profile value must be an object');
  }
  const unknown = Object.keys(value).find((key) => !allowedKeys.has(key));
  if (unknown) {
    refuse('AUTHORITY_SCHEMA_UNKNOWN_FIELD', `Unknown authority field ${unknown}`);
  }
  const missing = [...allowedKeys].find((key) => !Object.hasOwn(value, key));
  if (missing) {
    refuse('AUTHORITY_SCHEMA_INVALID', `Missing authority field ${missing}`);
  }
}

function digestValue(value) {
  return value?.value;
}

function sameSet(left, right) {
  return (
    Array.isArray(left) &&
    Array.isArray(right) &&
    left.length === right.length &&
    [...left].sort().every((value, index) => value === [...right].sort()[index])
  );
}

function sameArray(left, right) {
  return (
    Array.isArray(left) &&
    Array.isArray(right) &&
    left.length === right.length &&
    left.every((value, index) => value === right[index])
  );
}

function sameJson(left, right) {
  return canonicalJson(left) === canonicalJson(right);
}

function assertAuthentication(value, trusted) {
  const proof = trusted.authenticationProofs[value.authentication?.proofRef];
  if (!proof) {
    refuse(
      'AUTHORITY_AUTHENTICATION_UNVERIFIABLE',
      'Authority authentication proof is not independently verifiable'
    );
  }
  if (
    value.authentication?.method !== proof.method ||
    value.authentication?.keyId !== proof.keyId ||
    value.authentication?.signedDigest !== value.integrity?.value
  ) {
    refuse('AUTHORITY_AUTHENTICATION_INVALID', 'Authority authentication binding is invalid');
  }
  let publicKey;
  let signature;
  try {
    publicKey = createPublicKey({
      key: Buffer.from(proof.publicKeyDerHex, 'hex'),
      format: 'der',
      type: 'spki'
    });
    signature = Buffer.from(value.authentication.signature, 'hex');
  } catch {
    refuse('AUTHORITY_AUTHENTICATION_UNVERIFIABLE', 'Authority authentication key is invalid');
  }
  if (
    signature.toString('hex') !== value.authentication.signature ||
    !verifySignature(
      null,
      Buffer.from(value.authentication.signedDigest, 'hex'),
      publicKey,
      signature
    )
  ) {
    refuse('AUTHORITY_AUTHENTICATION_INVALID', 'Authority authentication signature is invalid');
  }
}

function assertIntegrity(value, target) {
  const expected = computeAuthorityDigest(value, target);
  if (digestValue(value.integrity) !== expected) {
    refuse('AUTHORITY_INTEGRITY_INVALID', 'Authority integrity digest mismatch');
  }
}

function assertBaseCorrelation(value, trusted) {
  if (
    value.principal?.principalId !== trusted.principalId ||
    value.principal?.authorizationProofRef !== trusted.principalAuthorizationProofRef ||
    value.principal?.authenticationState !== 'authenticated'
  ) {
    refuse('AUTHORITY_PRINCIPAL_MISMATCH', 'Authenticated principal does not match');
  }
  if (
    value.familiar?.familiarRootId !== trusted.familiarRootId ||
    value.familiar?.identityRevisionId !== trusted.identityRevisionId
  ) {
    refuse('AUTHORITY_FAMILIAR_MISMATCH', 'Familiar root or revision does not match');
  }
  if (value.familiar?.statusAtDecision !== 'active') {
    refuse('AUTHORITY_FAMILIAR_STATUS_INVALID', 'Familiar is not active at decision time');
  }
}

function assertCapabilities(value, trusted) {
  if (
    !Array.isArray(value.capabilities?.requested) ||
    !Array.isArray(value.capabilities?.granted) ||
    !Array.isArray(value.capabilities?.denied) ||
    !Array.isArray(value.capabilities?.degraded)
  ) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority capability sets must be arrays');
  }
  const requested = new Set(value.capabilities?.requested);
  if (
    !Array.isArray(value.capabilities?.granted) ||
    value.capabilities.granted.some(
      (capability) => !requested.has(capability) || !trusted.runtimeCapabilities.includes(capability)
    )
  ) {
    refuse('AUTHORITY_CAPABILITY_ESCALATION', 'Granted capability exceeds request or runtime');
  }
}

function assertFamiliarFreshness(value, trusted, dispatchNow, decisionAt) {
  if (
    value.familiar?.freshnessPolicyVersion !== trusted.familiarFreshnessPolicyVersion ||
    value.familiar?.freshnessBoundSeconds !== trusted.familiarFreshnessBoundSeconds
  ) {
    refuse('AUTHORITY_FAMILIAR_STALE', 'Familiar freshness policy does not match trusted state');
  }
  if (!isTimestamp(value.familiar?.verifiedAt)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Familiar verification timestamp is invalid');
  }
  const verifiedAt = Date.parse(value.familiar.verifiedAt);
  const validFrom = Date.parse(value.familiar?.validTime?.notBefore);
  const validUntil = Date.parse(value.familiar?.validTime?.notAfter);
  const revocationCheckedAt = Date.parse(value.familiar?.revocation?.checkedAt);
  const retirementCheckedAt = Date.parse(value.familiar?.retirement?.checkedAt);
  if (
    !isTimestamp(value.familiar?.validTime?.notBefore) ||
    !isTimestamp(value.familiar?.validTime?.notAfter) ||
    !isTimestamp(value.familiar?.revocation?.checkedAt) ||
    !isTimestamp(value.familiar?.retirement?.checkedAt)
  ) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Familiar validity timestamp is invalid');
  }
  if (
    value.familiar.validTime.notBefore !== trusted.familiarValidFrom ||
    value.familiar.validTime.notAfter !== trusted.familiarValidUntil ||
    value.familiar.revocation.state !== trusted.familiarRevocationState ||
    value.familiar.revocation.checkedAt !== trusted.familiarRevocationCheckedAt ||
    value.familiar.retirement.state !== trusted.familiarRetirementState ||
    value.familiar.retirement.checkedAt !== trusted.familiarRetirementCheckedAt
  ) {
    refuse('AUTHORITY_FAMILIAR_MISMATCH', 'Familiar validity evidence does not match');
  }
  if (
    validFrom > decisionAt ||
    decisionAt >= validUntil ||
    dispatchNow >= validUntil ||
    validFrom >= validUntil
  ) {
    refuse('AUTHORITY_FAMILIAR_STALE', 'Familiar validity does not cover dispatch');
  }
  if (
    revocationCheckedAt > decisionAt ||
    revocationCheckedAt > dispatchNow ||
    retirementCheckedAt > decisionAt ||
    retirementCheckedAt > dispatchNow
  ) {
    refuse(
      'AUTHORITY_FAMILIAR_TIME_INVALID',
      'Familiar lifecycle checks cannot follow decision or dispatch'
    );
  }
  if (verifiedAt > decisionAt || verifiedAt > dispatchNow) {
    refuse(
      'AUTHORITY_FAMILIAR_TIME_INVALID',
      'Familiar verification cannot follow the authority decision or dispatch'
    );
  }
  if (dispatchNow - verifiedAt > value.familiar.freshnessBoundSeconds * 1000) {
    refuse('AUTHORITY_FAMILIAR_STALE', 'Familiar verification exceeds its freshness bound');
  }
}

function recurringConsumptionKey(approval) {
  return canonicalJson({
    grantId: approval.use.grantId,
    requestDigest: digestValue(approval.consumption.requestDigest),
    decisionDigest: digestValue(approval.consumption.decisionDigest),
    occurrenceId: approval.consumption.occurrenceId,
    runId: approval.consumption.runId,
    attemptNumber: approval.consumption.attemptNumber,
    fenceGeneration: approval.consumption.fenceGeneration
  });
}

function expectedDispatchConsumption(value) {
  const approval =
    value.approval.requirement === 'not_required'
      ? null
      : {
          requirement: value.approval.requirement,
          approvalId: value.approval.evidence.approvalId,
          use: value.approval.use,
          consumption: value.approval.consumption
        };
  return {
    bindingId: value.bindingId,
    nonce: value.authorization.nonce,
    adoptionKey: value.base.adoptionKey,
    occurrenceId: value.base.occurrenceId,
    runId: value.base.runId,
    attemptId: value.base.attemptId,
    attemptNumber: value.base.attemptNumber,
    fenceGeneration: value.base.occurrenceFenceGeneration,
    approval
  };
}

function dispatchConsumptions(trusted) {
  return Array.isArray(trusted.dispatchConsumptions) ? trusted.dispatchConsumptions : [];
}

function assertPreDispatchReplayState(value, trusted) {
  const consumptions = dispatchConsumptions(trusted);
  if (
    trusted.replayedNonces.includes(value.authorization?.nonce) ||
    consumptions.some((entry) => entry.nonce === value.authorization?.nonce)
  ) {
    refuse('AUTHORITY_NONCE_REPLAYED', 'Authorization nonce was already adopted');
  }
  if (
    trusted.replayedAdoptionKeys.includes(value.base?.adoptionKey) ||
    consumptions.some((entry) => entry.adoptionKey === value.base?.adoptionKey)
  ) {
    refuse('AUTHORITY_ADOPTION_REPLAYED', 'Attempt adoption key was already adopted');
  }
}

function assertTerminalDispatchConsumption(value, trusted) {
  const recurringKey =
    value.approval.requirement === 'bounded_recurring'
      ? recurringConsumptionKey(value.approval)
      : null;
  const approvalRecorded =
    value.approval.requirement === 'not_required' ||
    (value.approval.requirement === 'bounded_recurring'
      ? trusted.consumedRecurringOccurrences.includes(recurringKey)
      : trusted.consumedApprovalIds.includes(value.approval.evidence.approvalId));
  if (
    !trusted.replayedNonces.includes(value.authorization.nonce) ||
    !trusted.replayedAdoptionKeys.includes(value.base.adoptionKey) ||
    !approvalRecorded
  ) {
    refuse(
      'AUTHORITY_DISPATCH_CONSUMPTION_MISSING',
      'Terminal authority evidence is missing a committed replay or approval record'
    );
  }

  const expected = expectedDispatchConsumption(value);
  const related = dispatchConsumptions(trusted).filter(
    (entry) =>
      entry.bindingId === expected.bindingId ||
      entry.nonce === expected.nonce ||
      entry.adoptionKey === expected.adoptionKey ||
      (entry.runId === expected.runId && entry.attemptNumber === expected.attemptNumber) ||
      (entry.occurrenceId === expected.occurrenceId &&
        entry.fenceGeneration === expected.fenceGeneration)
  );
  if (related.length === 0) {
    refuse(
      'AUTHORITY_DISPATCH_CONSUMPTION_MISSING',
      'Terminal authority evidence has no committed dispatch ownership record'
    );
  }
  if (related.length !== 1 || !sameJson(related[0], expected)) {
    refuse(
      'AUTHORITY_DISPATCH_CONSUMPTION_MISMATCH',
      'Committed dispatch ownership does not match the signed binding'
    );
  }
}

function assertApproval(value, trusted, dispatchNow, { enforceReplay = true } = {}) {
  const approval = value.approval;
  if (approval?.requirement !== trusted.approvalRequirement) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Approval requirement does not match trusted state');
  }
  if (approval.requirement === 'not_required') {
    return;
  }
  if (approval.evidence?.state === 'revoked') {
    refuse('AUTHORITY_APPROVAL_REVOKED', 'Approval is revoked');
  }
  if (!isTimestamp(approval.expiresAt) || Date.parse(approval.expiresAt) <= dispatchNow) {
    refuse('AUTHORITY_APPROVAL_EXPIRED', 'Approval is expired');
  }
  if (
    approval.evidence?.state !== 'approved' ||
    approval.evidence?.approvalId !== trusted.approvalId ||
    digestValue(approval.evidence?.approvalDigest) !== trusted.approvalDigest ||
    digestValue(approval.scopeDigest) !== trusted.approvalScopeDigest ||
    approval.expiresAt !== trusted.approvalExpiresAt ||
    digestValue(approval.consumption?.eventDigest) !== trusted.approvalConsumptionDigest ||
    approval.consumption?.state !== 'consumed_for_dispatch' ||
    digestValue(approval.consumption?.requestDigest) !==
      digestValue(value.authorization?.requestDigest) ||
    digestValue(approval.consumption?.decisionDigest) !==
      digestValue(value.authorization?.decisionDigest) ||
    approval.consumption?.occurrenceId !== value.base?.occurrenceId ||
    approval.consumption?.runId !== value.base?.runId ||
    approval.consumption?.attemptNumber !== value.base?.attemptNumber ||
    approval.consumption?.fenceGeneration !== value.base?.occurrenceFenceGeneration ||
    approval.consumption?.eventId !== trusted.approvalConsumption.eventId ||
    digestValue(approval.consumption?.eventDigest) !==
      trusted.approvalConsumption.eventDigest ||
    digestValue(approval.consumption?.requestDigest) !==
      trusted.approvalConsumption.requestDigest ||
    digestValue(approval.consumption?.decisionDigest) !==
      trusted.approvalConsumption.decisionDigest ||
    approval.consumption?.occurrenceId !== trusted.approvalConsumption.occurrenceId ||
    approval.consumption?.runId !== trusted.approvalConsumption.runId ||
    approval.consumption?.attemptNumber !== trusted.approvalConsumption.attemptNumber ||
    approval.consumption?.fenceGeneration !== trusted.approvalConsumption.fenceGeneration
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Approval evidence or consumption does not match dispatch');
  }

  if (approval.requirement === 'bounded_recurring') {
    if (
      approval.use?.kind !== 'recurring' ||
      approval.use.kind !== trusted.approvalUse.kind ||
      approval.use.grantId !== trusted.approvalUse.grantId ||
      approval.use.maxUses !== trusted.approvalUse.maxUses ||
      approval.use.occurrencePrefix !== trusted.approvalUse.occurrencePrefix
    ) {
      refuse('AUTHORITY_APPROVAL_REQUIRED', 'Recurring approval grant does not match trusted state');
    }
    if (!value.base.occurrenceId.startsWith(approval.use.occurrencePrefix)) {
      refuse(
        'AUTHORITY_APPROVAL_SCOPE_MISMATCH',
        'Recurring approval does not cover this occurrence'
      );
    }
    if (trusted.approvalUse.priorUses >= approval.use.maxUses) {
      refuse('AUTHORITY_APPROVAL_EXHAUSTED', 'Recurring approval usage bound is exhausted');
    }
    if (approval.use.priorUses !== trusted.approvalUse.priorUses) {
      refuse('AUTHORITY_APPROVAL_REUSED', 'Recurring approval usage snapshot is stale');
    }
    if (
      approval.consumption.usageNumber !== approval.use.priorUses + 1 ||
      approval.consumption.usageNumber !== trusted.approvalConsumption.usageNumber
    ) {
      refuse('AUTHORITY_APPROVAL_REUSED', 'Recurring approval usage is not monotonic');
    }
    if (
      enforceReplay &&
      (trusted.consumedRecurringOccurrences.includes(recurringConsumptionKey(approval)) ||
        dispatchConsumptions(trusted).some(
          (entry) =>
            entry.approval?.requirement === 'bounded_recurring' &&
            recurringConsumptionKey(entry.approval) === recurringConsumptionKey(approval)
        ))
    ) {
      refuse('AUTHORITY_APPROVAL_REUSED', 'Recurring approval already consumed this occurrence');
    }
    return;
  }

  if (
    approval.use?.kind !== 'single_use' ||
    !sameJson(approval.use, trusted.approvalUse) ||
    approval.consumption?.usageNumber !== undefined
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Per-run approval must be single use');
  }
  if (
    enforceReplay &&
    (trusted.consumedApprovalIds.includes(approval.evidence.approvalId) ||
      dispatchConsumptions(trusted).some(
        (entry) => entry.approval?.approvalId === approval.evidence.approvalId
      ))
  ) {
    refuse('AUTHORITY_APPROVAL_REUSED', 'Per-run approval was already consumed');
  }
}

function assertBinding(value, trusted, { phase = 'pre_dispatch' } = {}) {
  assertClosedTopLevel(value, BINDING_KEYS);
  if (value.kind !== 'AutomationExecutionBinding') {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority binding kind is invalid');
  }
  if (value.bindingId !== trusted.bindingId) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Binding identity does not match trusted state');
  }
  assertBaseCorrelation(value, trusted);
  if (
    value.base?.automationId !== trusted.automationId ||
    value.base?.automationRevision !== trusted.automationRevision ||
    digestValue(value.base?.definitionDigest) !== trusted.definitionDigest ||
    value.base?.occurrenceId !== trusted.occurrenceId ||
    value.base?.occurrenceKey !== trusted.occurrenceKey ||
    value.base?.runId !== trusted.runId ||
    value.base?.attemptId !== trusted.attemptId ||
    value.base?.attemptNumber !== trusted.attemptNumber ||
    value.base?.adoptionKey !== trusted.adoptionKey
  ) {
    refuse('AUTHORITY_DEFINITION_MISMATCH', 'Definition digest does not match');
  }
  if (value.base?.occurrenceFenceGeneration !== trusted.occurrenceFenceGeneration) {
    refuse('AUTHORITY_FENCE_STALE', 'Occurrence fence is stale');
  }
  if (phase === 'pre_dispatch') {
    assertPreDispatchReplayState(value, trusted);
  }
  if (value.authorization?.replayState !== 'fresh') {
    refuse('AUTHORITY_REPLAYED', 'Authority replay state is not fresh');
  }
  if (
    digestValue(value.authorization?.decisionDigest) !== trusted.threadsDecisionDigest ||
    digestValue(value.threads?.decisionDigest) !== trusted.threadsDecisionDigest ||
    value.authorization?.operation !== trusted.authorizationOperation ||
    value.authorization?.requestId !== trusted.authorizationRequestId ||
    digestValue(value.authorization?.requestDigest) !== trusted.authorizationRequestDigest ||
    digestValue(value.authorization?.consumptionSnapshotDigest) !==
      trusted.consumptionSnapshotDigest ||
    value.authorization?.outcome !== trusted.authorizationOutcome ||
    value.authorization?.decisionId !== value.threads?.decisionId ||
    value.threads?.protectedSurfaceManifestId !== trusted.protectedSurfaceManifestId ||
    digestValue(value.threads?.protectedSurfaceManifestDigest) !==
      trusted.protectedSurfaceManifestDigest
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Threads authority binding does not match');
  }
  assertCapabilities(value, trusted);
  if (
    !sameSet(value.capabilities?.requested, trusted.requestedCapabilities) ||
    !sameSet(value.capabilities?.granted, trusted.grantedCapabilities) ||
    !sameJson(value.capabilities?.denied, trusted.deniedCapabilities) ||
    !sameSet(value.capabilities?.degraded, trusted.degradedCapabilities)
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Capability decision does not match trusted state');
  }
  if (
    digestValue(value.familiar?.declarationDigest) !== trusted.familiarDeclarationDigest ||
    value.familiar?.embodimentBindingId !== trusted.familiarEmbodimentBindingId ||
    digestValue(value.familiar?.embodimentDigest) !== trusted.familiarEmbodimentDigest
  ) {
    refuse('AUTHORITY_FAMILIAR_MISMATCH', 'Familiar evidence digest does not match');
  }
  if (
    value.privacy?.sensitiveMaterialIncluded !== false
  ) {
    refuse(
      'AUTHORITY_EVIDENCE_PROJECTION_FORBIDDEN',
      'Authority binding contains unauthorized sensitive material'
    );
  }
  if (
    value.privacy?.classification !== trusted.privacyClassification ||
    value.privacy?.retention !== trusted.privacyRetention ||
    value.privacy?.redactionStatus !== trusted.privacyRedactionStatus
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Authority privacy classification does not match');
  }
  if (
    value.contextProjection?.projectId !== trusted.projectId ||
    value.contextProjection?.workspaceId !== trusted.workspaceId ||
    !sameArray(value.contextProjection?.contextProjectionIds, trusted.contextProjectionIds) ||
    !sameArray(value.contextProjection?.memoryProjectionIds, trusted.memoryProjectionIds)
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Authorized context projection does not match');
  }
  const dispatchNow = Date.parse(trusted.dispatchNow);
  if (
    !isTimestamp(value.authorization?.issuedAt) ||
    !isTimestamp(value.authorization?.validFrom) ||
    !isTimestamp(value.authorization?.validUntil)
  ) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authorization timestamp is invalid');
  }
  if (!isTimestamp(value.decisionTimestamp) || !Number.isFinite(dispatchNow)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority decision or dispatch timestamp is invalid');
  }
  const issuedAt = Date.parse(value.authorization.issuedAt);
  const validFrom = Date.parse(value.authorization.validFrom);
  const validUntil = Date.parse(value.authorization.validUntil);
  const decisionAt = Date.parse(value.decisionTimestamp);
  if (
    issuedAt > validFrom ||
    validFrom > decisionAt ||
    decisionAt > dispatchNow ||
    validFrom >= validUntil
  ) {
    refuse(
      'AUTHORITY_CHRONOLOGY_INVALID',
      'Authority timestamps must satisfy issuedAt <= validFrom <= decisionTimestamp <= dispatchNow < validUntil'
    );
  }
  assertFamiliarFreshness(value, trusted, dispatchNow, decisionAt);
  if (validUntil <= dispatchNow) {
    refuse('AUTHORITY_STALE', 'Authorization validity does not cover dispatch');
  }
  if (
    value.authorization?.outcome === 'requires_approval' &&
    value.approval?.requirement === 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Authorization requires approval evidence');
  }
  if (
    value.authorization?.outcome === 'permit' &&
    value.approval?.requirement !== 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Permit outcome cannot carry required approval evidence');
  }
  assertApproval(value, trusted, dispatchNow, { enforceReplay: phase === 'pre_dispatch' });
  if (
    value.runtime?.runtimeId !== trusted.runtimeId ||
    value.runtime?.descriptorVersion !== trusted.runtimeDescriptorVersion ||
    digestValue(value.runtime?.descriptorDigest) !== trusted.runtimeDescriptorDigest ||
    !sameSet(value.runtime?.capabilities, trusted.runtimeCapabilities) ||
    value.runtime?.selectionRationale !== trusted.runtimeSelectionRationale
  ) {
    refuse('AUTHORITY_RUNTIME_DOWNGRADE', 'Runtime descriptor or capabilities changed');
  }
  if (
    value.versions?.baseProfile !== 'coven.automations.v1' ||
    value.versions?.authorityProfile !== PROFILE ||
    value.versions?.familiarProfile !== 'familiar.embodiment_binding.v1' ||
    value.versions?.threadsProfile !== 'automation-authority/1.0.0' ||
    value.versions?.policyVersion !== trusted.policyVersion ||
    digestValue(value.versions?.policyDigest) !== trusted.policyDigest ||
    value.decisionTimestamp !== trusted.decisionTimestamp
  ) {
    refuse('AUTHORITY_POLICY_STALE', 'Policy snapshot is stale');
  }
  assertSchema(value, 'automation-execution-binding.schema.json');
  assertIntegrity(value, 'binding');
  assertAuthentication(value, trusted);
  if (digestValue(value.integrity) !== trusted.bindingDigest) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Binding digest does not match trusted state');
  }
  if (phase === 'terminal') {
    assertTerminalDispatchConsumption(value, trusted);
  }
}

function assertReceiptEvidence(value, trusted) {
  assertClosedTopLevel(value, RECEIPT_KEYS);
  if (value.kind !== 'AutomationReceiptAuthorityEvidence') {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority receipt evidence kind is invalid');
  }
  if (value.privacy?.sensitiveMaterialIncluded !== false) {
    refuse(
      'AUTHORITY_EVIDENCE_PROJECTION_FORBIDDEN',
      'Authority receipt evidence contains unauthorized sensitive material'
    );
  }
  if (
    value.authorization?.outcome === 'requires_approval' &&
    value.approval?.requirement === 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Receipt authorization requires approval evidence');
  }
  if (
    value.privacy?.classification !== trusted.privacyClassification ||
    value.privacy?.retention !== trusted.privacyRetention ||
    value.privacy?.redactionStatus !== trusted.privacyRedactionStatus
  ) {
    refuse(
      'AUTHORITY_RECEIPT_BINDING_MISMATCH',
      'Receipt privacy classification does not match'
    );
  }
  if (
    value.authorization?.outcome === 'permit' &&
    value.approval?.requirement !== 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Receipt permit cannot carry required approval evidence');
  }
  assertSchema(value, 'automation-receipt-authority-evidence.schema.json');
  if (value.principalId !== trusted.principalId) {
    refuse('AUTHORITY_PRINCIPAL_MISMATCH', 'Receipt principal does not match');
  }
  if (
    value.familiar?.familiarRootId !== trusted.familiarRootId ||
    value.familiar?.identityRevisionId !== trusted.identityRevisionId
  ) {
    refuse('AUTHORITY_FAMILIAR_MISMATCH', 'Receipt familiar binding does not match');
  }
  if (value.familiar?.statusAtDecision !== 'active') {
    refuse('AUTHORITY_FAMILIAR_STATUS_INVALID', 'Receipt familiar status is not active');
  }
  const dispatchNow = Date.parse(trusted.dispatchNow);
  const decisionAt = Date.parse(value.decisionTimestamp);
  if (!Number.isFinite(dispatchNow) || !Number.isFinite(decisionAt) || decisionAt > dispatchNow) {
    refuse('AUTHORITY_CHRONOLOGY_INVALID', 'Receipt decision cannot follow dispatch');
  }
  assertFamiliarFreshness(value, trusted, dispatchNow, decisionAt);
  if (
    value.receiptId !== trusted.receiptId ||
    digestValue(value.baseReceiptDigest) !== trusted.baseReceiptDigest ||
    value.bindingId !== trusted.bindingId ||
    digestValue(value.bindingDigest) !== trusted.bindingDigest
  ) {
    refuse(
      'AUTHORITY_RECEIPT_CORRELATION_MISMATCH',
      'Receipt evidence does not match the authenticated base receipt'
    );
  }
  if (
    value.automationId !== trusted.automationId ||
    value.automationRevision !== trusted.automationRevision ||
    digestValue(value.definitionDigest) !== trusted.definitionDigest ||
    value.occurrenceId !== trusted.occurrenceId ||
    value.occurrenceFenceGeneration !== trusted.occurrenceFenceGeneration ||
    value.runId !== trusted.runId ||
    value.attemptId !== trusted.attemptId ||
    value.attemptNumber !== trusted.attemptNumber
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Receipt base correlation does not match');
  }
  assertCapabilities(value, trusted);
  if (
    !sameSet(value.capabilities?.requested, trusted.requestedCapabilities) ||
    !sameSet(value.capabilities?.granted, trusted.grantedCapabilities) ||
    !sameJson(value.capabilities?.denied, trusted.deniedCapabilities) ||
    !sameSet(value.capabilities?.degraded, trusted.degradedCapabilities)
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Receipt capability decision does not match');
  }
  if (
    value.capabilities?.exercised.some(
      (capability) => !value.capabilities.granted.includes(capability)
    )
  ) {
    refuse(
      'AUTHORITY_CAPABILITY_ESCALATION',
      'Receipt claims a capability outside its granted capability set'
    );
  }
  if (
    value.runtime?.runtimeId !== trusted.runtimeId ||
    value.runtime?.descriptorVersion !== trusted.runtimeDescriptorVersion ||
    digestValue(value.runtime?.descriptorDigest) !== trusted.runtimeDescriptorDigest ||
    !sameSet(value.runtime?.capabilities, trusted.runtimeCapabilities)
  ) {
    refuse('AUTHORITY_RUNTIME_DOWNGRADE', 'Receipt runtime snapshot changed');
  }
  assertApproval(
    {
      approval: value.approval,
      authorization: value.authorization,
      base: {
        occurrenceId: value.occurrenceId,
        runId: value.runId,
        attemptNumber: value.attemptNumber,
        occurrenceFenceGeneration: value.occurrenceFenceGeneration
      }
    },
    trusted,
    dispatchNow,
    { enforceReplay: false }
  );
  if (
    digestValue(value.familiar?.declarationDigest) !== trusted.familiarDeclarationDigest ||
    value.authorization?.operation !== trusted.authorizationOperation ||
    value.authorization?.requestId !== trusted.authorizationRequestId ||
    digestValue(value.authorization?.requestDigest) !== trusted.authorizationRequestDigest ||
    digestValue(value.authorization?.decisionDigest) !== trusted.threadsDecisionDigest ||
    digestValue(value.authorization?.consumptionSnapshotDigest) !==
      trusted.consumptionSnapshotDigest ||
    value.authorization?.outcome !== trusted.authorizationOutcome ||
    value.risk?.riskClass !== trusted.riskClass ||
    value.risk?.sideEffectClass !== trusted.sideEffectClass ||
    value.decisionTimestamp !== trusted.decisionTimestamp
  ) {
    refuse('AUTHORITY_BINDING_MISMATCH', 'Receipt authority correlation does not match');
  }
  if (!isTimestamp(value.decisionTimestamp)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Receipt decision timestamp is invalid');
  }
  assertIntegrity(value, 'receiptEvidence');
  assertAuthentication(value, trusted);
}

function assertExtension(value, trusted, phase) {
  assertClosedTopLevel(value, EXTENSION_KEYS);
  if (value.kind !== 'AutomationAuthorityExtension') {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Authority extension kind is invalid');
  }
  if (phase !== undefined && !['pre_dispatch', 'terminal'].includes(phase)) {
    refuse('AUTHORITY_SCHEMA_INVALID', `Unknown authority validation phase ${phase}`);
  }
  const terminal =
    phase === 'terminal' ||
    (phase === undefined &&
      (value.receiptEvidence !== null ||
        trusted.receiptId !== null ||
        trusted.baseReceiptDigest !== null));
  if (phase === 'pre_dispatch' && value.receiptEvidence !== null) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Pre-dispatch authority cannot contain receipt evidence');
  }
  assertBinding(value.executionBinding, trusted, {
    phase: terminal ? 'terminal' : 'pre_dispatch'
  });
  if (value.receiptEvidence === null) {
    if (
      terminal ||
      trusted.receiptId !== null ||
      trusted.baseReceiptDigest !== null
    ) {
      refuse(
        'AUTHORITY_RECEIPT_EVIDENCE_REQUIRED',
        'Terminal receipt state requires authenticated authority evidence'
      );
    }
    assertSchema(value, 'authority-extension.schema.json');
    return;
  }
  const binding = value.executionBinding;
  const receipt = value.receiptEvidence;
  assertProfile(receipt);
  assertClosedTopLevel(receipt, RECEIPT_KEYS);
  if (receipt.privacy?.sensitiveMaterialIncluded !== false) {
    refuse(
      'AUTHORITY_EVIDENCE_PROJECTION_FORBIDDEN',
      'Authority receipt evidence contains unauthorized sensitive material'
    );
  }
  if (
    receipt.authorization?.outcome === 'requires_approval' &&
    receipt.approval?.requirement === 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Receipt authorization requires approval evidence');
  }
  if (
    receipt.authorization?.outcome === 'permit' &&
    receipt.approval?.requirement !== 'not_required'
  ) {
    refuse('AUTHORITY_APPROVAL_REQUIRED', 'Receipt permit cannot carry required approval evidence');
  }
  assertSchema(receipt, 'automation-receipt-authority-evidence.schema.json');
  if (
    receipt.receiptId !== trusted.receiptId ||
    digestValue(receipt.baseReceiptDigest) !== trusted.baseReceiptDigest ||
    receipt.bindingId !== binding.bindingId ||
    digestValue(receipt.bindingDigest) !== binding.integrity.value
  ) {
    refuse(
      receipt.receiptId !== trusted.receiptId ||
      digestValue(receipt.baseReceiptDigest) !== trusted.baseReceiptDigest
        ? 'AUTHORITY_RECEIPT_CORRELATION_MISMATCH'
        : 'AUTHORITY_RECEIPT_BINDING_MISMATCH',
      'Receipt evidence is not correlated to the trusted receipt and binding'
    );
  }
  if (
    receipt.capabilities.exercised.some(
      (capability) =>
        !receipt.capabilities.granted.includes(capability) ||
        !binding.capabilities.granted.includes(capability)
    )
  ) {
    refuse(
      'AUTHORITY_CAPABILITY_ESCALATION',
      'Receipt claims a capability that was not granted by both evidence and binding'
    );
  }
  if (
    receipt.automationId !== binding.base.automationId ||
    receipt.automationRevision !== binding.base.automationRevision ||
    digestValue(receipt.definitionDigest) !== digestValue(binding.base.definitionDigest) ||
    receipt.occurrenceId !== binding.base.occurrenceId ||
    receipt.occurrenceFenceGeneration !== binding.base.occurrenceFenceGeneration ||
    receipt.runId !== binding.base.runId ||
    receipt.attemptId !== binding.base.attemptId ||
    receipt.attemptNumber !== binding.base.attemptNumber ||
    receipt.principalId !== binding.principal.principalId ||
    receipt.familiar.familiarRootId !== binding.familiar.familiarRootId ||
    receipt.familiar.identityRevisionId !== binding.familiar.identityRevisionId ||
    digestValue(receipt.familiar.declarationDigest) !==
      digestValue(binding.familiar.declarationDigest) ||
    receipt.familiar.statusAtDecision !== binding.familiar.statusAtDecision ||
    receipt.familiar.verifiedAt !== binding.familiar.verifiedAt ||
    receipt.familiar.freshnessPolicyVersion !== binding.familiar.freshnessPolicyVersion ||
    receipt.familiar.freshnessBoundSeconds !== binding.familiar.freshnessBoundSeconds ||
    receipt.authorization.operation !== binding.authorization.operation ||
    receipt.authorization.requestId !== binding.authorization.requestId ||
    digestValue(receipt.authorization.requestDigest) !==
      digestValue(binding.authorization.requestDigest) ||
    receipt.authorization.decisionId !== binding.authorization.decisionId ||
    digestValue(receipt.authorization.decisionDigest) !==
      digestValue(binding.authorization.decisionDigest) ||
    digestValue(receipt.authorization.consumptionSnapshotDigest) !==
      digestValue(binding.authorization.consumptionSnapshotDigest) ||
    receipt.authorization.outcome !== binding.authorization.outcome ||
    !sameSet(receipt.capabilities.requested, binding.capabilities.requested) ||
    !sameSet(receipt.capabilities.granted, binding.capabilities.granted) ||
    !sameJson(receipt.capabilities.denied, binding.capabilities.denied) ||
    !sameSet(receipt.capabilities.degraded, binding.capabilities.degraded) ||
    !sameJson(receipt.approval, binding.approval) ||
    receipt.risk.riskClass !== binding.risk.riskClass ||
    receipt.risk.sideEffectClass !== binding.risk.sideEffectClass ||
    receipt.runtime.runtimeId !== binding.runtime.runtimeId ||
    receipt.runtime.descriptorVersion !== binding.runtime.descriptorVersion ||
    digestValue(receipt.runtime.descriptorDigest) !==
      digestValue(binding.runtime.descriptorDigest) ||
    !sameSet(receipt.runtime.capabilities, binding.runtime.capabilities) ||
    receipt.decisionTimestamp !== binding.decisionTimestamp ||
    !sameJson(receipt.producer, binding.producer) ||
    !sameJson(receipt.privacy, binding.privacy)
  ) {
    refuse('AUTHORITY_RECEIPT_BINDING_MISMATCH', 'Receipt evidence was spliced');
  }
  assertReceiptEvidence(receipt, trusted);
  assertSchema(value, 'authority-extension.schema.json');
}

export function validateAuthorityValue(value, trusted, target, { phase } = {}) {
  assertIJson(value);
  assertProfile(value);
  assertTrustedState(trusted);
  if (target === 'binding') {
    assertBinding(value, trusted);
  } else if (target === 'receiptEvidence') {
    assertReceiptEvidence(value, trusted);
  } else if (target === 'extension') {
    assertExtension(value, trusted, phase);
  } else {
    refuse('AUTHORITY_SCHEMA_INVALID', `Unknown vector target ${target}`);
  }
  return value;
}

export function negotiateAuthorityProfile({
  consumerClass,
  advertisedProfiles,
  advertisedCapabilities,
  extensions,
  trusted,
  phase = 'terminal'
}) {
  assertIJson(extensions);
  if (!isPlainObject(extensions)) {
    refuse('AUTHORITY_SCHEMA_INVALID', 'Extensions must be an object');
  }
  if (consumerClass === 'generic-base-v1') {
    return {
      disposition: 'preserved-opaque',
      extensions: structuredClone(extensions)
    };
  }
  if (consumerClass !== 'runtime-authority-v1') {
    refuse('AUTHORITY_PROFILE_UNKNOWN', `Unknown authority consumer ${consumerClass}`);
  }
  if (
    !Array.isArray(advertisedProfiles) ||
    !advertisedProfiles.includes(BASE_PROFILE) ||
    !advertisedProfiles.includes(PROFILE) ||
    !Array.isArray(advertisedCapabilities) ||
    !advertisedCapabilities.includes(RUNTIME_AUTHORITY_CAPABILITY)
  ) {
    refuse(
      'AUTHORITY_PROFILE_REQUIRED',
      'Runtime Authority requires explicit profile and capability advertisement'
    );
  }
  if (!Object.hasOwn(extensions, AUTHORITY_EXTENSION_KEY)) {
    refuse('AUTHORITY_PROFILE_MISSING', 'Authority extension is missing');
  }
  const authority = extensions[AUTHORITY_EXTENSION_KEY];
  validateAuthorityValue(authority, trusted, 'extension', { phase });
  return {
    disposition: 'validated',
    authority
  };
}

function pointerComponents(pointer) {
  if (typeof pointer !== 'string' || !pointer.startsWith('/')) {
    refuse('AUTHORITY_SCHEMA_INVALID', `Invalid mutation path ${pointer}`);
  }
  return pointer
    .slice(1)
    .split('/')
    .map((component) => component.replaceAll('~1', '/').replaceAll('~0', '~'));
}

function applyMutation(value, mutation) {
  if (!mutation) {
    return value;
  }
  const components = pointerComponents(mutation.path);
  if (mutation.op === 'addEncodedKey') {
    let target = value;
    for (const component of components) {
      target = target[component];
    }
    if (
      mutation.encoding !== 'utf16-code-unit' ||
      !/^[0-9a-f]{4}$/i.test(mutation.codeUnit) ||
      !isPlainObject(target)
    ) {
      refuse('AUTHORITY_SCHEMA_INVALID', 'Invalid encoded object-key mutation');
    }
    target[String.fromCharCode(Number.parseInt(mutation.codeUnit, 16))] = mutation.value;
    return value;
  }
  if (mutation.op === 'addArrayProperty' || mutation.op === 'addSymbolKey') {
    let target = value;
    for (const component of components) {
      target = target[component];
    }
    if (mutation.op === 'addArrayProperty') {
      if (!Array.isArray(target) || typeof mutation.key !== 'string') {
        refuse('AUTHORITY_SCHEMA_INVALID', 'Invalid array-property mutation');
      }
      target[mutation.key] = mutation.value;
    } else {
      if (!isPlainObject(target) || typeof mutation.key !== 'string') {
        refuse('AUTHORITY_SCHEMA_INVALID', 'Invalid symbol-key mutation');
      }
      target[Symbol(mutation.key)] = mutation.value;
    }
    return value;
  }
  const last = components.pop();
  let parent = value;
  for (const component of components) {
    parent = parent[component];
  }
  if (mutation.op === 'replaceSparseArray') {
    const sparse = [];
    sparse.length = 2;
    sparse[1] = mutation.value;
    parent[last] = sparse;
  } else if (mutation.op === 'replaceNonJson') {
    const replacements = {
      nan: Number.NaN,
      undefined,
      date: new Date(0)
    };
    if (!Object.hasOwn(replacements, mutation.kind)) {
      refuse('AUTHORITY_SCHEMA_INVALID', 'Invalid non-JSON mutation');
    }
    parent[last] = replacements[mutation.kind];
  } else if (mutation.op === 'replaceEncodedString') {
    if (
      mutation.encoding !== 'utf16-code-unit' ||
      !/^[0-9a-f]{4}$/i.test(mutation.codeUnit)
    ) {
      refuse('AUTHORITY_SCHEMA_INVALID', 'Invalid encoded string mutation');
    }
    parent[last] = String.fromCharCode(Number.parseInt(mutation.codeUnit, 16));
  } else if (mutation.op === 'remove') {
    delete parent[last];
  } else if (mutation.op === 'replace') {
    parent[last] = mutation.value;
  } else if (mutation.op === 'add' && last === '-' && Array.isArray(parent)) {
    parent.push(mutation.value);
  } else if (mutation.op === 'add') {
    parent[last] = mutation.value;
  } else {
    refuse('AUTHORITY_SCHEMA_INVALID', `Unsupported mutation ${mutation.op}`);
  }
  return value;
}

function vectorFixture(vectors, target) {
  if (target === 'extension') {
    return {
      profile: PROFILE,
      kind: 'AutomationAuthorityExtension',
      executionBinding: structuredClone(vectors.fixtures.binding),
      receiptEvidence: structuredClone(vectors.fixtures.receiptEvidence)
    };
  }
  return structuredClone(vectors.fixtures[target]);
}

function mergeTrusted(base, patch = {}) {
  return {
    ...structuredClone(base),
    ...structuredClone(patch)
  };
}

export function runAuthorityVectors(vectors) {
  let accepted = 0;
  let refused = 0;
  for (const vector of vectors.cases) {
    let value = Object.hasOwn(vector, 'input')
      ? structuredClone(vector.input)
      : vectorFixture(vectors, vector.target);
    for (const mutation of vector.mutations ?? (vector.mutation ? [vector.mutation] : [])) {
      value = applyMutation(value, mutation);
    }
    const trustedState =
      vector.trustedState ?? (vector.target === 'extension' ? 'terminalRecurring' : undefined);
    let trusted = mergeTrusted(
      vectors.trusted,
      trustedState ? vectors.trustedStates?.[trustedState] : undefined
    );
    trusted = mergeTrusted(trusted, vector.trustedPatch);
    for (const mutation of vector.trustedMutations ?? (vector.trustedMutation ? [vector.trustedMutation] : [])) {
      trusted = applyMutation(trusted, mutation);
    }
    try {
      validateAuthorityValue(value, trusted, vector.target);
      if (vector.expected !== 'accept') {
        throw new Error(`${vector.id}: expected refusal ${vector.errorCode}, got accept`);
      }
      accepted += 1;
    } catch (error) {
      if (vector.expected !== 'refuse') {
        throw new Error(`${vector.id}: expected accept, got ${error.code ?? error.message}`);
      }
      if (error.code !== vector.errorCode) {
        throw new Error(
          `${vector.id}: expected ${vector.errorCode}, got ${error.code ?? error.message}`
        );
      }
      refused += 1;
    }
  }
  for (const vector of vectors.negotiationCases ?? []) {
    let extensions = vector.extensionFixture
      ? {
          [AUTHORITY_EXTENSION_KEY]: {
            profile: PROFILE,
            kind: 'AutomationAuthorityExtension',
            executionBinding: structuredClone(vectors.fixtures.binding),
            receiptEvidence: structuredClone(vectors.fixtures.receiptEvidence)
          }
        }
      : structuredClone(vector.extensions);
    for (const mutation of vector.mutations ?? (vector.mutation ? [vector.mutation] : [])) {
      extensions = applyMutation(extensions, mutation);
    }
    let trusted = mergeTrusted(
      vectors.trusted,
      vector.trustedState ? vectors.trustedStates?.[vector.trustedState] : undefined
    );
    trusted = mergeTrusted(trusted, vector.trustedPatch);
    for (const mutation of vector.trustedMutations ?? []) {
      trusted = applyMutation(trusted, mutation);
    }
    try {
      const result = negotiateAuthorityProfile({
        consumerClass: vector.consumerClass,
        advertisedProfiles: vector.advertisedProfiles,
        advertisedCapabilities: vector.advertisedCapabilities,
        extensions,
        trusted,
        phase: vector.phase
      });
      if (vector.expected !== 'accept') {
        throw new Error(`${vector.id}: expected refusal ${vector.errorCode}, got accept`);
      }
      if (
        vector.consumerClass === 'generic-base-v1' &&
        !sameJson(result.extensions, vector.extensions)
      ) {
        throw new Error(`${vector.id}: generic consumer changed opaque extensions`);
      }
      accepted += 1;
    } catch (error) {
      if (vector.expected !== 'refuse') {
        throw new Error(`${vector.id}: expected accept, got ${error.code ?? error.message}`);
      }
      if (error.code !== vector.errorCode) {
        throw new Error(
          `${vector.id}: expected ${vector.errorCode}, got ${error.code ?? error.message}`
        );
      }
      refused += 1;
    }
  }
  return {
    total: vectors.cases.length + (vectors.negotiationCases?.length ?? 0),
    accepted,
    refused
  };
}

function main() {
  const repositoryRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
  const vectorsPath = path.join(
    repositoryRoot,
    'spec',
    'coven-automations',
    'authority',
    'v1',
    'test-vectors.json'
  );
  const vectors = JSON.parse(readFileSync(vectorsPath, 'utf8'));
  process.stdout.write(`${JSON.stringify(runAuthorityVectors(vectors))}\n`);
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? '').href) {
  try {
    main();
  } catch (error) {
    console.error(error?.message ?? String(error));
    process.exitCode = 1;
  }
}
