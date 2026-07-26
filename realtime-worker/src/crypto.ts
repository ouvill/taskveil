import {
  KEY_ID_PATTERN,
  MAX_PUBLISH_SKEW_SECONDS,
  MAX_TICKET_BYTES,
  OPAQUE_TAG_PATTERN,
  TICKET_AUDIENCE,
  TICKET_TTL_SECONDS,
  type PublishPayload,
  type TicketPayload,
} from "./contracts";
import { readPublishBody } from "./body";

const decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false });
const encoder = new TextEncoder();

interface KeySlot {
  id: string;
  encodedKey: string;
}

export interface SigningKeySet {
  current: KeySlot;
  previous: KeySlot;
}

export interface VerifiedPublish {
  payload: PublishPayload;
  body: Uint8Array;
}

export function isValidSigningKeySet(keys: SigningKeySet): boolean {
  if (
    !KEY_ID_PATTERN.test(keys.current.id) ||
    !KEY_ID_PATTERN.test(keys.previous.id) ||
    keys.current.id === keys.previous.id
  ) {
    return false;
  }
  return isEncoded32ByteKey(keys.current.encodedKey) &&
    isEncoded32ByteKey(keys.previous.encodedKey);
}

export async function verifyTicket(
  token: string,
  keys: SigningKeySet,
  nowSeconds: number,
): Promise<TicketPayload | null> {
  if (encoder.encode(token).byteLength > MAX_TICKET_BYTES) return null;
  const segments = token.split(".");
  if (segments.length !== 2) return null;
  const payloadSegment = segments[0];
  const signatureSegment = segments[1];
  if (!payloadSegment || !signatureSegment) return null;

  const payloadBytes = decodeBase64Url(payloadSegment);
  const signature = decodeBase64Url(signatureSegment);
  if (!payloadBytes || !signature || signature.byteLength !== 32) return null;

  let candidate: unknown;
  let payloadText: string;
  try {
    payloadText = decoder.decode(payloadBytes);
    if (!bytesEqual(encoder.encode(payloadText), payloadBytes)) return null;
    candidate = JSON.parse(payloadText);
  } catch {
    return null;
  }
  if (!isTicketPayload(candidate)) return null;
  if (payloadText !== JSON.stringify(candidate)) return null;

  const slot = selectKey(keys, candidate.kid);
  if (!slot) return null;
  const signatureInput = encoder.encode(
    `taskveil-realtime-ticket-v1\n${payloadSegment}`,
  );
  if (!(await verifyHmac(slot.encodedKey, signatureInput, signature))) {
    return null;
  }

  if (candidate.exp - candidate.iat !== TICKET_TTL_SECONDS) return null;
  if (candidate.iat > nowSeconds + MAX_PUBLISH_SKEW_SECONDS) return null;
  if (candidate.exp <= nowSeconds) return null;
  return candidate;
}

export async function verifyPublish(
  request: Request,
  keys: SigningKeySet,
  nowSeconds: number,
): Promise<VerifiedPublish | null> {
  const body = await readPublishBody(request);
  if (!body) return null;

  const kid = request.headers.get("X-Taskveil-Realtime-Key-Id");
  const timestampText = request.headers.get("X-Taskveil-Realtime-Timestamp");
  const signatureText = request.headers.get("X-Taskveil-Realtime-Signature");
  if (!kid || !timestampText || !signatureText || !KEY_ID_PATTERN.test(kid)) {
    return null;
  }
  const timestamp = parseCanonicalInteger(timestampText);
  if (
    timestamp === null ||
    Math.abs(nowSeconds - timestamp) > MAX_PUBLISH_SKEW_SECONDS
  ) {
    return null;
  }
  const slot = selectKey(keys, kid);
  const signature = decodeBase64Url(signatureText);
  if (!slot || !signature || signature.byteLength !== 32) return null;

  const prefix = encoder.encode(`taskveil-realtime-publish-v1\n${timestampText}\n`);
  const signatureInput = new Uint8Array(prefix.byteLength + body.byteLength);
  signatureInput.set(prefix);
  signatureInput.set(body, prefix.byteLength);
  if (!(await verifyHmac(slot.encodedKey, signatureInput, signature))) {
    return null;
  }

  let candidate: unknown;
  let bodyText: string;
  try {
    bodyText = decoder.decode(body);
    if (!bytesEqual(encoder.encode(bodyText), body)) return null;
    candidate = JSON.parse(bodyText);
  } catch {
    return null;
  }
  if (!isPublishPayload(candidate) || bodyText !== JSON.stringify(candidate)) {
    return null;
  }
  return { body, payload: candidate };
}

function selectKey(keys: SigningKeySet, kid: string): KeySlot | null {
  if (keys.current.id === kid) return keys.current;
  if (keys.previous.id === kid) return keys.previous;
  return null;
}

async function verifyHmac(
  encodedKey: string,
  input: Uint8Array,
  signature: Uint8Array,
): Promise<boolean> {
  const rawKey = decodeBase64Url(encodedKey);
  if (!rawKey || rawKey.byteLength !== 32) return false;
  const key = await crypto.subtle.importKey(
    "raw",
    rawKey,
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["verify"],
  );
  return crypto.subtle.verify("HMAC", key, signature, input);
}

function isTicketPayload(value: unknown): value is TicketPayload {
  if (!isRecord(value)) return false;
  if (!hasExactKeys(value, ["kid", "aud", "channel", "device", "iat", "exp"])) {
    return false;
  }
  return (
    typeof value.kid === "string" &&
    KEY_ID_PATTERN.test(value.kid) &&
    value.aud === TICKET_AUDIENCE &&
    typeof value.channel === "string" &&
    isOpaqueTag(value.channel) &&
    typeof value.device === "string" &&
    isOpaqueTag(value.device) &&
    Number.isSafeInteger(value.iat) &&
    Number.isSafeInteger(value.exp)
  );
}

function isPublishPayload(value: unknown): value is PublishPayload {
  if (!isRecord(value)) return false;
  if (!hasExactKeys(value, ["v", "channel", "source_device"])) return false;
  return (
    value.v === 1 &&
    typeof value.channel === "string" &&
    isOpaqueTag(value.channel) &&
    typeof value.source_device === "string" &&
    isOpaqueTag(value.source_device)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function hasExactKeys(
  value: Record<string, unknown>,
  expected: readonly string[],
): boolean {
  const actual = Object.keys(value);
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function parseCanonicalInteger(value: string): number | null {
  if (!/^(0|[1-9][0-9]*)$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

export function decodeBase64Url(value: string): Uint8Array | null {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) return null;
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  try {
    const binary = atob(value.replaceAll("-", "+").replaceAll("_", "/") + padding);
    const decoded = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    return encodeBase64Url(decoded) === value ? decoded : null;
  } catch {
    return null;
  }
}

export function isOpaqueTag(value: string): boolean {
  return OPAQUE_TAG_PATTERN.test(value) && decodeBase64Url(value)?.byteLength === 32;
}

function encodeBase64Url(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) return false;
  return left.every((byte, index) => byte === right[index]);
}

function isEncoded32ByteKey(value: string): boolean {
  return decodeBase64Url(value)?.byteLength === 32;
}
