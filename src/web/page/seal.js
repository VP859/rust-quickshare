// End-to-end encryption of a browser session, as src/web/seal.rs describes it:
// a stream is an 8-byte random prefix and records of 64 KiB of plaintext, each
// encrypted with ChaCha20-Poly1305; the nonce of record i is the prefix and
// i (with the top bit set for the last record) as a big-endian u32.
//
// ChaCha20-Poly1305 comes from @noble/ciphers (vendor/, unchanged copies of
// the npm package; scripts/check-frontend.mjs checks that they are): browsers
// offer WebCrypto only to HTTPS pages, and the session runs over plain HTTP.
import { chacha20poly1305 } from './vendor/chacha.js';

export const RECORD = 64 * 1024;
export const TAG = 16;
export const PREFIX = 8;
const LAST = 0x80000000;

export const purpose = {
  info: 'ferry-web-v1 info',
  offer: 'ferry-web-v1 offer',
  download: (index) => `ferry-web-v1 download ${index}`,
  upload: (offer, index) => `ferry-web-v1 upload ${offer} ${index}`,
};

const utf8 = (text) => new TextEncoder().encode(text);

/** The key in the link fragment (`#k=<URL-safe base64>`), or null. */
export function keyFromFragment(fragment) {
  const match = /(?:^|[#&])k=([A-Za-z0-9_-]{43})(?:&|$)/.exec(fragment ?? '');
  if (!match) return null;
  const b64 = match[1].replace(/-/g, '+').replace(/_/g, '/') + '=';
  const bytes = Uint8Array.from(atob(b64), (c) => c.charCodeAt(0));
  return bytes.length === 32 ? bytes : null;
}

const records = (len) => Math.max(1, Math.ceil(len / RECORD));

/** Length of the stream of `len` bytes of plaintext. */
export const sealedLen = (len) => PREFIX + len + TAG * records(len);

/** Length of the plaintext in a stream of `len` bytes, or null. */
export function plainLen(len) {
  const body = len - PREFIX;
  if (body < 0) return null;
  const count = Math.max(1, Math.ceil(body / (RECORD + TAG)));
  const plain = body - count * TAG;
  return plain >= 0 && sealedLen(plain) === len ? plain : null;
}

function nonce(prefix, index, last) {
  if (index >= LAST) throw new Error('stream too long');
  const n = new Uint8Array(12);
  n.set(prefix);
  new DataView(n.buffer).setUint32(PREFIX, (index | (last ? LAST : 0)) >>> 0);
  return n;
}

/** Encrypts a stream record by record. */
export class Sealer {
  constructor(key, aad, prefix = crypto.getRandomValues(new Uint8Array(PREFIX))) {
    this.key = key;
    this.aad = utf8(aad);
    this.prefix = prefix;
    this.index = 0;
  }

  /** The next record (at most RECORD bytes) with its tag. */
  seal(plain, last) {
    if (plain.length > RECORD) throw new Error('record too long');
    const sealed = chacha20poly1305(this.key, nonce(this.prefix, this.index, last), this.aad).encrypt(plain);
    this.index += 1;
    return sealed;
  }
}

/** Encrypts a whole message (a JSON text) as one stream. */
export function sealAll(key, aad, plain, prefix) {
  const sealer = new Sealer(key, aad, prefix);
  const parts = [sealer.prefix];
  const count = records(plain.length);
  for (let i = 0; i < count; i++) parts.push(sealer.seal(plain.subarray(i * RECORD, (i + 1) * RECORD), i + 1 === count));
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}

/** Decrypts a stream of a known plaintext length as its bytes arrive. */
export class Opener {
  constructor(key, aad, plainLength) {
    this.key = key;
    this.aad = utf8(aad);
    this.prefix = null;
    this.pending = new Uint8Array(0);
    this.left = plainLength;
    this.index = 0;
    this.done = false;
  }

  /** Takes the next bytes; returns the plaintext of the records they complete. */
  push(data) {
    const joined = new Uint8Array(this.pending.length + data.length);
    joined.set(this.pending);
    joined.set(data, this.pending.length);
    let at = 0;
    const out = [];
    if (!this.prefix) {
      if (joined.length < PREFIX) { this.pending = joined; return out; }
      this.prefix = joined.slice(0, PREFIX);
      at = PREFIX;
    }
    while (true) {
      if (this.done) {
        if (at < joined.length) throw new Error('data after the end of the stream');
        break;
      }
      const plain = Math.min(this.left, RECORD);
      const last = this.left <= RECORD;
      if (joined.length - at < plain + TAG) break;
      const record = joined.subarray(at, at + plain + TAG);
      // Throws when the record was changed, reordered or is not ours.
      out.push(chacha20poly1305(this.key, nonce(this.prefix, this.index, last), this.aad).decrypt(record));
      at += plain + TAG;
      this.index += 1;
      this.left -= plain;
      this.done = last;
    }
    this.pending = joined.slice(at);
    return out;
  }

  /** Throws when the stream ended early. */
  finish() {
    if (!this.done || this.pending.length) throw new Error('the encrypted data ended early');
  }
}

/** Decrypts a whole message made by sealAll. */
export function openAll(key, aad, sealed) {
  const len = plainLen(sealed.length);
  if (len === null) throw new Error('not an encrypted message');
  const opener = new Opener(key, aad, len);
  const parts = opener.push(sealed);
  opener.finish();
  const out = new Uint8Array(len);
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
}
