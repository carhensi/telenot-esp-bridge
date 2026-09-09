import { sha256 } from '@noble/hashes/sha2.js';

// The setup UI is served over HTTP, where crypto.subtle is unavailable.
export function firmwareSha256(buffer) {
  return Array.from(sha256(new Uint8Array(buffer)), (b) => b.toString(16).padStart(2, '0')).join('');
}
