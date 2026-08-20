/**
 * Key material lives outside the repository.
 *
 * In production the coven signing key is hardware-backed (Secure Enclave or a
 * hardware token) and Val is the only holder. In this local build it is a
 * 0600 file in a directory the repo cannot reach, which preserves the property
 * that matters for the demo: an agent editing repository files cannot produce
 * a valid signature.
 */

import { generateKeyPairSync, sign as edSign, createPrivateKey } from 'node:crypto';
import { mkdirSync, readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

export interface KeyRecord {
  keyId: string;
  publicKey: string; // base64url raw 32 bytes
  privateKey?: string; // base64url raw seed-bearing PKCS8, present only locally
  createdAt: string;
}

export function keystoreDir(): string {
  const override = process.env.BOUND_KEYSTORE;
  if (override && override.trim() !== '') return override;
  return join(homedir(), '.coven', 'workspaces', 'familiars', 'cody', '.bound', 'keys');
}

function keyPath(keyId: string): string {
  return join(keystoreDir(), `${keyId}.json`);
}

export function generateKey(keyId: string): KeyRecord {
  const dir = keystoreDir();
  mkdirSync(dir, { recursive: true, mode: 0o700 });

  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const rawPublic = publicKey.export({ type: 'spki', format: 'der' }).subarray(-32);
  const pkcs8 = privateKey.export({ type: 'pkcs8', format: 'der' });

  const record: KeyRecord = {
    keyId,
    publicKey: Buffer.from(rawPublic).toString('base64url'),
    privateKey: Buffer.from(pkcs8).toString('base64url'),
    createdAt: new Date().toISOString(),
  };

  writeFileSync(keyPath(keyId), `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600 });
  return record;
}

export function loadKey(keyId: string): KeyRecord | null {
  const path = keyPath(keyId);
  if (!existsSync(path)) return null;
  return JSON.parse(readFileSync(path, 'utf8')) as KeyRecord;
}

export function ensureKey(keyId: string): KeyRecord {
  return loadKey(keyId) ?? generateKey(keyId);
}

export function listKeys(): KeyRecord[] {
  const dir = keystoreDir();
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => f.endsWith('.json'))
    .map((f) => JSON.parse(readFileSync(join(dir, f), 'utf8')) as KeyRecord);
}

/** Trusted public keys, as the authority and gateway see them. */
export function trustedKeys(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const key of listKeys()) out[key.keyId] = key.publicKey;
  return out;
}

export function signBytes(record: KeyRecord, bytes: Buffer): string {
  if (!record.privateKey) throw new Error(`key ${record.keyId} has no private half available`);
  const key = createPrivateKey({
    key: Buffer.from(record.privateKey, 'base64url'),
    format: 'der',
    type: 'pkcs8',
  });
  return edSign(null, bytes, key).toString('base64url');
}
