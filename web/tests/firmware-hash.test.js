import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { firmwareSha256 } from '../src/firmware-hash.js';

test('OTA hash works without Web Crypto and matches known SHA-256 vectors', () => {
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'crypto');
  Object.defineProperty(globalThis, 'crypto', { value: undefined, configurable: true });
  try {
    assert.equal(firmwareSha256(new ArrayBuffer(0)),
      'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855');
    assert.equal(firmwareSha256(new TextEncoder().encode('abc').buffer),
      'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad');
    const bytes = Uint8Array.from({ length: 4 * 1024 * 1024 }, (_, i) => i % 251);
    assert.equal(firmwareSha256(bytes.buffer), createHash('sha256').update(bytes).digest('hex'));
  } finally {
    if (descriptor) Object.defineProperty(globalThis, 'crypto', descriptor);
    else delete globalThis.crypto;
  }
});
