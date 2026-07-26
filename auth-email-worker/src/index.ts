import { EmailMessage } from "cloudflare:email";

const MAX_COMMAND_BYTES = 8 * 1024;
const SIGNATURE_WINDOW_SECONDS = 300;
const INGRESS_PATH = "/v1/enqueue";
const DELIVERY_ID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}:[1-9][0-9]{0,8}$/;
const SEALED_MAGIC = new TextEncoder().encode("TVE1");
const PAYLOAD_PURPOSE = new TextEncoder().encode(
  "taskveil/email/delivery-payload/v1\0",
);
export { EmailDeliveryLedger } from "./delivery-ledger";

interface DeliveryPayload {
  recipient: string;
  otp: string;
  template: "verify-email-otp-v1";
}

class TerminalCommandError extends Error {}

export default {
  async fetch(request: Request, env: CloudflareBindings): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path !== INGRESS_PATH) {
      return new Response("not found", { status: 404 });
    }
    if (request.method !== "POST") {
      return new Response("method not allowed", {
        headers: { Allow: "POST" },
        status: 405,
      });
    }
    const bodyResult = await readBoundedBody(request, MAX_COMMAND_BYTES);
    if (bodyResult === "too-large") {
      return new Response("payload too large", { status: 413 });
    }
    if (bodyResult === "invalid") {
      return new Response("invalid request", { status: 400 });
    }
    const body = bodyResult;
    if (
      body.byteLength === 0 ||
      !(await verifyIngress(request.method, path, request.headers, body, env))
    ) {
      return new Response("unauthorized", { status: 401 });
    }
    const command = parseCommand(body);
    if (!command) return new Response("invalid command", { status: 400 });
    if (Date.parse(command.not_after) <= Date.now()) {
      return new Response("invalid command", { status: 400 });
    }
    let claim: Awaited<ReturnType<typeof claimIngress>>;
    try {
      claim = await claimIngress(command, env);
    } catch {
      return new Response("temporarily unavailable", {
        headers: { "Retry-After": "30" },
        status: 503,
      });
    }
    if (claim === "enqueued") return new Response(null, { status: 202 });
    if (typeof claim !== "string" && claim.status === "leased") {
      return new Response("temporarily unavailable", {
        headers: { "Retry-After": String(claim.retryAfterSeconds) },
        status: 503,
      });
    }
    try {
      await env.EMAIL_COMMANDS.send(command, {
        contentType: "json",
      });
      await completeIngress(command.delivery_id, env);
    } catch {
      // Do not release the ingress lease here. A Queue send can succeed while
      // its response is lost, so immediate reuse could amplify one command
      // without bound. The lease permits one bounded retry after uncertainty.
      return new Response("temporarily unavailable", {
        headers: { "Retry-After": "300" },
        status: 503,
      });
    }
    console.log(JSON.stringify({ event: "auth_email_queue_accepted" }));
    return new Response(null, { status: 202 });
  },

  async queue(
    batch: MessageBatch<EmailDeliveryCommand>,
    env: CloudflareBindings,
  ): Promise<void> {
    for (const message of batch.messages) {
      const command = parseCommand(
        new TextEncoder().encode(JSON.stringify(message.body)),
      );
      if (!command || Date.parse(command.not_after) <= Date.now()) {
        console.log(JSON.stringify({ event: "auth_email_delivery_terminal" }));
        message.ack();
        continue;
      }
      try {
        const claim = await claimDelivery(command, env);
        if (claim === "accepted") {
          message.ack();
          continue;
        }
        if (typeof claim !== "string" && claim.status === "leased") {
          await deferLeasedDelivery(
            message,
            command,
            claim.retryAfterSeconds,
            env,
          );
          continue;
        }
        const payload = await decryptPayload(command, env);
        validatePayload(payload);
        await env.SEND_EMAIL.send(
          emailMessage(
            env.EMAIL_FROM,
            payload.recipient,
            payload.otp,
            command.delivery_id,
          ),
        );
        await completeDelivery(command.delivery_id, env);
        console.log(JSON.stringify({ event: "auth_email_service_accepted" }));
        message.ack();
      } catch (error) {
        // Keep the lease after any attempted Email Service submission. A
        // rejected promise can be an ambiguous response loss; releasing here
        // would allow unbounded duplicate mail. Queue retry resumes after the
        // bounded lease, while an accepted state suppresses it permanently.
        if (error instanceof TerminalCommandError) {
          console.log(JSON.stringify({ event: "auth_email_delivery_terminal" }));
          message.ack();
        } else {
          console.log(JSON.stringify({ event: "auth_email_delivery_retryable" }));
          message.retry({ delaySeconds: 300 });
        }
      }
    }
  },
} satisfies ExportedHandler<CloudflareBindings, EmailDeliveryCommand>;

async function claimDelivery(
  command: EmailDeliveryCommand,
  env: CloudflareBindings,
): Promise<
  | "send"
  | "accepted"
  | { status: "leased"; retryAfterSeconds: number }
> {
  if (env.TEST_ONLY_DISABLE_LEDGER === "enabled") return "send";
  const response = await deliveryLedger(env, command.delivery_id).fetch(
    "https://delivery.internal/delivery/claim",
    claimRequest(command.not_after),
  );
  if (response.status === 201) return "send";
  if (response.status === 200) return "accepted";
  if (response.status === 409) {
    return {
      status: "leased",
      retryAfterSeconds: parseRetryAfter(response.headers),
    };
  }
  throw new Error("ledger unavailable");
}

async function completeDelivery(
  deliveryId: string,
  env: CloudflareBindings,
): Promise<void> {
  if (env.TEST_ONLY_DISABLE_LEDGER === "enabled") return;
  const response = await deliveryLedger(env, deliveryId).fetch(
    "https://delivery.internal/delivery/complete",
    { method: "POST" },
  );
  if (!response.ok) throw new Error("ledger unavailable");
}

async function claimIngress(
  command: EmailDeliveryCommand,
  env: CloudflareBindings,
): Promise<
  | "enqueue"
  | "enqueued"
  | { status: "leased"; retryAfterSeconds: number }
> {
  if (env.TEST_ONLY_DISABLE_LEDGER === "enabled") return "enqueue";
  const response = await deliveryLedger(env, command.delivery_id).fetch(
    "https://delivery.internal/ingress/claim",
    claimRequest(command.not_after),
  );
  if (response.status === 201) return "enqueue";
  if (response.status === 200) return "enqueued";
  if (response.status === 409) {
    return {
      status: "leased",
      retryAfterSeconds: parseRetryAfter(response.headers),
    };
  }
  throw new Error("ledger unavailable");
}

async function deferLeasedDelivery(
  message: Message<EmailDeliveryCommand>,
  command: EmailDeliveryCommand,
  retryAfterSeconds: number,
  env: CloudflareBindings,
): Promise<void> {
  if (Date.now() + retryAfterSeconds * 1000 >= Date.parse(command.not_after)) {
    console.log(JSON.stringify({ event: "auth_email_delivery_terminal" }));
    message.ack();
    return;
  }
  try {
    // A live Durable Object lease means this consumer did not attempt Email
    // Service delivery. Publishing a delayed replacement and acknowledging
    // this message preserves the Queue attempt budget for real delivery
    // failures. The delivery ledger still suppresses duplicate replacements.
    await env.EMAIL_COMMANDS.send(command, {
      contentType: "json",
      delaySeconds: retryAfterSeconds,
    });
    message.ack();
  } catch {
    // Preserve at-least-once delivery if replacement publication fails. This
    // exceptional infrastructure failure consumes a Queue retry, unlike the
    // normal lease-defer path.
    message.retry({ delaySeconds: retryAfterSeconds });
  }
}

async function completeIngress(
  deliveryId: string,
  env: CloudflareBindings,
): Promise<void> {
  if (env.TEST_ONLY_DISABLE_LEDGER === "enabled") return;
  const response = await deliveryLedger(env, deliveryId).fetch(
    "https://delivery.internal/ingress/complete",
    { method: "POST" },
  );
  if (!response.ok) throw new Error("ledger unavailable");
}

function claimRequest(notAfter: string): RequestInit {
  return {
    body: JSON.stringify({ not_after: notAfter }),
    headers: { "content-type": "application/json" },
    method: "POST",
  };
}

function deliveryLedger(
  env: CloudflareBindings,
  deliveryId: string,
): DurableObjectStub {
  return env.EMAIL_DELIVERY_LEDGER.jurisdiction("eu").getByName(deliveryId);
}

async function verifyIngress(
  method: string,
  path: string,
  headers: Headers,
  body: Uint8Array,
  env: CloudflareBindings,
): Promise<boolean> {
  const keyId = headers.get("x-taskveil-key-id");
  const timestampText = headers.get("x-taskveil-timestamp");
  const signature = headers.get("x-taskveil-signature");
  if (!keyId || !timestampText || !signature || !/^[0-9]{10}$/.test(timestampText)) {
    return false;
  }
  const timestamp = Number(timestampText);
  const now = Math.floor(Date.now() / 1000);
  if (!Number.isSafeInteger(timestamp) || Math.abs(now - timestamp) > SIGNATURE_WINDOW_SECONDS) {
    return false;
  }
  const encodedKey =
    keyId === env.DELIVERY_SIGNING_KEY_CURRENT_ID
      ? env.DELIVERY_SIGNING_KEY_CURRENT
      : keyId === env.DELIVERY_SIGNING_KEY_PREVIOUS_ID
        ? env.DELIVERY_SIGNING_KEY_PREVIOUS
        : undefined;
  if (!encodedKey) return false;
  const keyBytes = decodeBase64(encodedKey);
  if (keyBytes.byteLength !== 32) return false;
  const bodyDigest = await crypto.subtle.digest("SHA-256", body);
  const input = new TextEncoder().encode(
    `${method}\n${path}\n${keyId}\n${timestampText}\n${encodeBase64Url(new Uint8Array(bodyDigest))}`,
  );
  const key = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    { hash: "SHA-256", name: "HMAC" },
    false,
    ["verify"],
  );
  let decodedSignature: Uint8Array;
  try {
    decodedSignature = decodeBase64Url(signature);
  } catch {
    return false;
  }
  return crypto.subtle.verify("HMAC", key, decodedSignature, input);
}

async function readBoundedBody(
  request: Request,
  limit: number,
): Promise<Uint8Array | "too-large" | "invalid"> {
  const contentLength = request.headers.get("content-length");
  if (contentLength !== null) {
    if (!/^[0-9]+$/.test(contentLength)) return "invalid";
    if (BigInt(contentLength) > BigInt(limit)) {
      try {
        await request.body?.cancel("payload too large");
      } catch {
        // The response remains 413 even when the producer ignores cancellation.
      }
      return "too-large";
    }
  }

  if (!request.body) return new Uint8Array();
  const reader = request.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      const chunk = result.value;
      if (length + chunk.byteLength > limit) {
        try {
          await reader.cancel("payload too large");
        } catch {
          // The size result is authoritative even if cancellation fails.
        }
        return "too-large";
      }
      chunks.push(chunk);
      length += chunk.byteLength;
    }
  } catch {
    return "invalid";
  } finally {
    reader.releaseLock();
  }

  const body = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

function parseCommand(body: Uint8Array): EmailDeliveryCommand | undefined {
  try {
    const value: unknown = JSON.parse(
      new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(body),
    );
    if (
      typeof value !== "object" ||
      value === null ||
      !hasExactKeys(value, [
        "version",
        "delivery_id",
        "not_after",
        "encrypted_payload",
      ]) ||
      (value as EmailDeliveryCommand).version !== 1 ||
      typeof (value as EmailDeliveryCommand).delivery_id !== "string" ||
      !DELIVERY_ID.test((value as EmailDeliveryCommand).delivery_id) ||
      typeof (value as EmailDeliveryCommand).not_after !== "string" ||
      !Number.isFinite(Date.parse((value as EmailDeliveryCommand).not_after)) ||
      typeof (value as EmailDeliveryCommand).encrypted_payload !== "string" ||
      (value as EmailDeliveryCommand).encrypted_payload.length > 4096
    ) {
      return undefined;
    }
    return value as EmailDeliveryCommand;
  } catch {
    return undefined;
  }
}

function hasExactKeys(value: object, expected: string[]): boolean {
  const keys = Object.keys(value).sort();
  const allowed = [...expected].sort();
  return (
    keys.length === allowed.length &&
    keys.every((key, index) => key === allowed[index])
  );
}

async function decryptPayload(
  command: EmailDeliveryCommand,
  env: CloudflareBindings,
): Promise<unknown> {
  const sealed = decodeBase64Url(command.encrypted_payload);
  if (sealed.byteLength < 36 || !equal(sealed.subarray(0, 4), SEALED_MAGIC)) {
    throw new Error("invalid ciphertext");
  }
  const view = new DataView(sealed.buffer, sealed.byteOffset, sealed.byteLength);
  const version = view.getUint32(4);
  const currentVersion = parseVersion(env.DATA_KEY_CURRENT_VERSION);
  const previousVersion = parseVersion(env.DATA_KEY_PREVIOUS_VERSION);
  const encodedKey =
    version === currentVersion
      ? env.DATA_KEY_CURRENT
      : version === previousVersion
        ? env.DATA_KEY_PREVIOUS
        : undefined;
  if (!encodedKey) throw new Error("unknown key");
  const keyBytes = decodeBase64(encodedKey);
  if (keyBytes.byteLength !== 32) throw new Error("invalid key");
  const nonce = sealed.subarray(8, 20);
  const ciphertext = sealed.subarray(20);
  const aad = encryptionAad(version, deliveryBinding(command.delivery_id));
  const key = await crypto.subtle.importKey(
    "raw",
    keyBytes,
    "AES-GCM",
    false,
    ["decrypt"],
  );
  const plaintext = await crypto.subtle.decrypt(
    { additionalData: aad, iv: nonce, name: "AES-GCM", tagLength: 128 },
    key,
    ciphertext,
  );
  return JSON.parse(
    new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(plaintext),
  );
}

function validatePayload(payload: unknown): asserts payload is DeliveryPayload {
  if (
    typeof payload !== "object" ||
    payload === null ||
    Array.isArray(payload)
  ) {
    throw new TerminalCommandError("invalid payload");
  }
  const candidate = payload as Partial<DeliveryPayload>;
  const keys = Object.keys(payload).sort();
  if (
    keys.length !== 3 ||
    keys[0] !== "otp" ||
    keys[1] !== "recipient" ||
    keys[2] !== "template" ||
    candidate.template !== "verify-email-otp-v1" ||
    typeof candidate.recipient !== "string" ||
    !validAddress(candidate.recipient) ||
    typeof candidate.otp !== "string" ||
    !/^[0-9]{8}$/.test(candidate.otp)
  ) {
    throw new TerminalCommandError("invalid payload");
  }
}

function emailMessage(
  from: string,
  to: string,
  otp: string,
  deliveryId: string,
): EmailMessage {
  if (!validAddress(from) || !validAddress(to) || !/^[0-9]{8}$/.test(otp)) {
    throw new TerminalCommandError("invalid email address");
  }
  const messageDomain = from.split("@").at(-1);
  if (!messageDomain) throw new TerminalCommandError("invalid sender");
  const subject = "Your Taskveil verification code";
  const text = [
    "Enter this verification code in Taskveil:",
    "",
    otp,
    "",
    "If you did not request this code, you can safely ignore this email.",
    "This confirms mailbox access only. It cannot recover encrypted Taskveil data.",
  ].join("\r\n");
  const raw = [
    `From: ${from}`,
    `To: ${to}`,
    `Subject: ${subject}`,
    `Message-ID: <${deliveryId.replace(":", ".")}@${messageDomain}>`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=UTF-8",
    "Content-Transfer-Encoding: 8bit",
    "",
    text,
  ].join("\r\n");
  return new EmailMessage(from, to, raw);
}

function validAddress(value: string): boolean {
  if (value.length === 0 || value.length > 254 || /[\r\n]/.test(value)) {
    return false;
  }
  const parts = value.split("@");
  if (parts.length !== 2) return false;
  const [local, domain] = parts;
  if (
    !local ||
    local.length > 64 ||
    local.startsWith(".") ||
    local.endsWith(".") ||
    local.includes("..") ||
    !/^[A-Za-z0-9!#$%&'*+\-/=?^_`{|}~.]+$/.test(local) ||
    !domain ||
    domain.length > 253 ||
    domain.endsWith(".")
  ) {
    return false;
  }
  return domain.split(".").every(
    (label) =>
      label.length > 0 &&
      label.length <= 63 &&
      !label.startsWith("-") &&
      !label.endsWith("-") &&
      /^[A-Za-z0-9-]+$/.test(label),
  );
}

function deliveryBinding(deliveryId: string): Uint8Array {
  const [uuid, generationText] = deliveryId.split(":");
  if (!uuid || !generationText) throw new Error("invalid delivery ID");
  const hex = uuid.replaceAll("-", "");
  const binding = new Uint8Array(20);
  for (let index = 0; index < 16; index += 1) {
    binding[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  new DataView(binding.buffer).setInt32(16, Number(generationText));
  return binding;
}

function encryptionAad(version: number, binding: Uint8Array): Uint8Array {
  const prefix = new TextEncoder().encode("taskveil/email-verification/aead/v1\0");
  const aad = new Uint8Array(prefix.length + 4 + PAYLOAD_PURPOSE.length + 1 + binding.length);
  let offset = 0;
  aad.set(prefix, offset);
  offset += prefix.length;
  new DataView(aad.buffer).setUint32(offset, version);
  offset += 4;
  aad.set(PAYLOAD_PURPOSE, offset);
  offset += PAYLOAD_PURPOSE.length;
  aad[offset] = 0;
  aad.set(binding, offset + 1);
  return aad;
}

function parseVersion(value: string): number | undefined {
  return /^[1-9][0-9]*$/.test(value) ? Number(value) : undefined;
}

function equal(left: Uint8Array, right: Uint8Array): boolean {
  if (left.byteLength !== right.byteLength) return false;
  let difference = 0;
  for (let index = 0; index < left.byteLength; index += 1) {
    difference |= left[index]! ^ right[index]!;
  }
  return difference === 0;
}

function decodeBase64(value: string): Uint8Array {
  return Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
}

function decodeBase64Url(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]+$/.test(value)) throw new Error("invalid base64url");
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  return decodeBase64(value.replaceAll("-", "+").replaceAll("_", "/") + padding);
}

function encodeBase64Url(value: Uint8Array): string {
  let binary = "";
  for (const byte of value) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function parseRetryAfter(headers: Headers): number {
  const value = headers.get("retry-after")?.trim();
  if (value && /^[0-9]+$/.test(value)) {
    return clampRetryAfter(Number(value));
  }
  if (value) {
    const date = Date.parse(value);
    if (Number.isFinite(date)) {
      return clampRetryAfter(Math.ceil((date - Date.now()) / 1000));
    }
  }
  return 300;
}

function clampRetryAfter(value: number): number {
  if (!Number.isSafeInteger(value)) return 300;
  return Math.min(24 * 60 * 60, Math.max(1, value));
}
