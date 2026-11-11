#!/usr/bin/env node
// Independent synthetic wire fixtures using Node/OpenSSL primitives, not snow,
// ring or the Rust protocol encoder. Noise NK follows Noise spec revision 34,
// sections 5 (symmetric state), 7 (NK pattern), and 12 (ChaChaPoly/SHA256).
import { createHash, createHmac, createPrivateKey, createPublicKey, diffieHellman,
  createCipheriv, sign, verify } from 'node:crypto';
import { readFileSync, writeFileSync, existsSync } from 'node:fs';
import assert from 'node:assert/strict';

const here = new URL('.', import.meta.url);
const path = new URL('channel-v2.json', here);
const wire = JSON.parse(readFileSync(new URL('wire-v2.json', here)));
const v1 = JSON.parse(readFileSync(new URL('wire.json', here)));
const concat = (...values) => Buffer.concat(values);
const hex = value => Buffer.from(value, 'hex');
const hash = value => createHash('sha256').update(value).digest();
const hmac = (key, data) => createHmac('sha256', key).update(data).digest();
function hkdf2(key, input) {
  const extracted = hmac(key, input);
  const first = hmac(extracted, Buffer.from([1]));
  return [first, hmac(extracted, concat(first, Buffer.from([2])))];
}
function aead(key, counter, aad, plaintext) {
  const nonce = Buffer.alloc(12);
  nonce.writeBigUInt64LE(BigInt(counter), 4);
  const cipher = createCipheriv('chacha20-poly1305', key, nonce, { authTagLength: 16 });
  cipher.setAAD(aad, { plaintextLength: plaintext.length });
  return concat(cipher.update(plaintext), cipher.final(), cipher.getAuthTag());
}
function xkey(bytes) {
  return createPrivateKey({ key: concat(hex('302e020100300506032b656e04220420'), bytes), format: 'der', type: 'pkcs8' });
}
function xpublic(key) {
  return createPublicKey(key).export({ format: 'der', type: 'spki' }).subarray(-32);
}
function dh(key, publicBytes) {
  const publicKey = createPublicKey({ key: concat(hex('302a300506032b656e032100'), publicBytes), format: 'der', type: 'spki' });
  return diffieHellman({ privateKey: key, publicKey });
}
function pkey(scalar) {
  const bytes = Buffer.alloc(32); bytes[31] = scalar;
  return createPrivateKey({ key: concat(hex('30310201010420'), bytes, hex('a00a06082a8648ce3d030107')), format: 'der', type: 'sec1' });
}
function transcript(transport) {
  const hostPrivate = Buffer.alloc(32, 9);
  const initiatorPrivate = Buffer.alloc(32, 11);
  const responderPrivate = Buffer.alloc(32, 13);
  const host = xkey(hostPrivate), initiator = xkey(initiatorPrivate), responder = xkey(responderPrivate);
  const hostPublic = xpublic(host), ie = xpublic(initiator), re = xpublic(responder);
  const name = Buffer.from('Noise_NK_25519_ChaChaPoly_SHA256');
  let h = name.length <= 32 ? concat(name, Buffer.alloc(32 - name.length)) : hash(name);
  let ck = Buffer.from(h), key;
  const mixHash = bytes => { h = hash(concat(h, bytes)); };
  const mixKey = input => { [ck, key] = hkdf2(ck, input); };
  const encrypt = plaintext => { const ciphertext = aead(key, 0, h, plaintext); mixHash(ciphertext); return ciphertext; };
  const prologue = hex(v1.prologues[transport]);
  mixHash(prologue);
  mixHash(hostPublic); // <- s (responder pre-message)
  mixHash(ie); mixKey(dh(initiator, hostPublic)); // -> e, es
  const first = concat(ie, encrypt(Buffer.alloc(0)));
  mixHash(re); mixKey(dh(responder, ie)); // <- e, ee
  const second = concat(re, encrypt(Buffer.alloc(0)));
  const [initiatorKey, responderKey] = hkdf2(ck, Buffer.alloc(0));
  const plaintext = Buffer.from('synthetic v2 transport interoperability');
  return { transport, hostPrivate: hostPrivate.toString('hex'), hostPublic: hostPublic.toString('hex'),
    initiatorEphemeral: initiatorPrivate.toString('hex'), responderEphemeral: responderPrivate.toString('hex'),
    prologue: prologue.toString('hex'), first: first.toString('hex'), second: second.toString('hex'),
    handshakeHash: h.toString('hex'), plaintext: plaintext.toString('hex'),
    initiatorRecord: aead(initiatorKey, 0, Buffer.alloc(0), plaintext).toString('hex'),
    responderRecord: aead(responderKey, 0, Buffer.alloc(0), plaintext).toString('hex') };
}
function fixture(previous) {
  const signatures = [];
  for (const [index, proof] of wire.possessionProofs.entries()) {
    for (const [role, scalar] of [['host', 1], ['device', 2]]) {
      const preimage = hex(proof[`${role}Proof`]);
      const key = pkey(scalar);
      const old = previous?.signatures.find(value => value.case === index && value.role === role);
      const signature = old ? hex(old.signature) : sign('sha256', preimage, { key, dsaEncoding: 'ieee-p1363' });
      const options = { key: createPublicKey(key), dsaEncoding: 'ieee-p1363' };
      assert(verify('sha256', preimage, options, signature), 'independent positive signature rejected');
      const corrupt = Buffer.from(signature); corrupt[0] ^= 1;
      assert(!verify('sha256', preimage, options, corrupt), 'negative signature accepted');
      const other = hex(proof[`${role === 'host' ? 'device' : 'host'}Proof`]);
      assert(!verify('sha256', other, options, signature), 'wrong-role preimage accepted');
      signatures.push({ case: index, role, preimage: preimage.toString('hex'), signature: signature.toString('hex'),
        invalidSignature: corrupt.toString('hex'), wrongRolePreimage: other.toString('hex') });
    }
  }
  return { fixtureFormatVersion: 1, routingProtocolVersion: 2, possessionProofVersion: 2,
    noiseProtocol: 'Noise_NK_25519_ChaChaPoly_SHA256',
    notice: 'Synthetic independent Node/OpenSSL signatures and full two-message Noise NK transcripts. Not provider, physical-device, hosted, or security-review acceptance.',
    signatures, noise: ['direct', 'relay'].map(transcript) };
}
const mode = process.argv[2];
assert(['--write', '--check'].includes(mode) && process.argv.length === 3, 'use --write or --check');
const old = existsSync(path) ? JSON.parse(readFileSync(path)) : undefined;
const rendered = JSON.stringify(fixture(old), null, 2) + '\n';
if (mode === '--write') writeFileSync(path, rendered);
else assert.equal(readFileSync(path, 'utf8'), rendered, 'channel fixture differs');
console.log('Independent v2 signature and Noise fixture check passed.');
