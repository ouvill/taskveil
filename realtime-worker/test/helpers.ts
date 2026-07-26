import { env } from "cloudflare:workers";
import { SELF } from "cloudflare:test";
import type { PublishPayload, TicketPayload } from "../src/contracts";

const encoder = new TextEncoder();

export const CHANNEL = opaqueTag(0);
export const DEVICE_A = opaqueTag(1);
export const DEVICE_B = opaqueTag(2);

export async function connect(
  device: string,
  options: { channel?: string; expiresAt?: number; kid?: string; key?: string } = {},
): Promise<Response> {
  const now = Math.floor(Date.now() / 1000);
  const token = await issueTicket({
    kid: options.kid ?? env.TICKET_KEY_CURRENT_ID,
    aud: "taskveil-realtime",
    channel: options.channel ?? CHANNEL,
    device,
    iat: (options.expiresAt ?? now + 300) - 300,
    exp: options.expiresAt ?? now + 300,
  }, options.key ?? env.TICKET_KEY_CURRENT);
  return SELF.fetch("https://example.com/v1/connect", {
    headers: { Authorization: `Bearer ${token}`, Upgrade: "websocket" },
  });
}

export async function issueTicket(
  payload: TicketPayload,
  encodedKey: string,
): Promise<string> {
  const payloadSegment = encodeBase64Url(encoder.encode(JSON.stringify(payload)));
  const input = encoder.encode(`taskveil-realtime-ticket-v1\n${payloadSegment}`);
  const signature = await sign(encodedKey, input);
  return `${payloadSegment}.${encodeBase64Url(signature)}`;
}

export async function publish(
  payload: PublishPayload,
  options: { kid?: string; key?: string; timestamp?: number; body?: Uint8Array } = {},
): Promise<Response> {
  const timestamp = options.timestamp ?? Math.floor(Date.now() / 1000);
  const body = options.body ?? encoder.encode(JSON.stringify(payload));
  const signature = await signPublishBody(
    options.key ?? env.PUBLISH_KEY_CURRENT,
    timestamp,
    body,
  );
  return SELF.fetch("https://example.com/v1/publish", {
    body,
    headers: {
      "Content-Length": String(body.byteLength),
      "Content-Type": "application/json",
      "X-Taskveil-Realtime-Key-Id": options.kid ?? env.PUBLISH_KEY_CURRENT_ID,
      "X-Taskveil-Realtime-Signature": signature,
      "X-Taskveil-Realtime-Timestamp": String(timestamp),
    },
    method: "POST",
  });
}

export async function signPublishBody(
  encodedKey: string,
  timestamp: number,
  body: Uint8Array,
): Promise<string> {
  const prefix = encoder.encode(`taskveil-realtime-publish-v1\n${timestamp}\n`);
  const input = new Uint8Array(prefix.byteLength + body.byteLength);
  input.set(prefix);
  input.set(body, prefix.byteLength);
  return encodeBase64Url(await sign(encodedKey, input));
}

export async function signFixtureInput(
  encodedKey: string,
  input: string,
): Promise<string> {
  return encodeBase64Url(await sign(encodedKey, encoder.encode(input)));
}

export function message(socket: WebSocket): Promise<string> {
  return new Promise((resolve) => {
    socket.addEventListener("message", (event) => resolve(event.data as string), {
      once: true,
    });
  });
}

export function closeEvent(socket: WebSocket): Promise<CloseEvent> {
  return new Promise((resolve) => {
    socket.addEventListener("close", resolve, { once: true });
  });
}

export function opaqueTag(byte: number): string {
  return encodeBase64Url(new Uint8Array(32).fill(byte));
}

async function sign(encodedKey: string, input: Uint8Array): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey(
    "raw",
    decodeBase64Url(encodedKey),
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["sign"],
  );
  return new Uint8Array(await crypto.subtle.sign("HMAC", key, input));
}

function encodeBase64Url(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function decodeBase64Url(value: string): Uint8Array {
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  const binary = atob(value.replaceAll("-", "+").replaceAll("_", "/") + padding);
  return Uint8Array.from(binary, (character) => character.charCodeAt(0));
}
