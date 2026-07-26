import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { EmailMessage } from "cloudflare:email";
import worker from "../src/index";
import { EmailDeliveryLedger } from "../src/delivery-ledger";

const encoder = new TextEncoder();
const signingKey = new Uint8Array(32).fill(3);
const dataKey = new Uint8Array(32).fill(4);
const deliveryId = "019f9d8a-e8ca-7031-afb0-c11ab745caa9:1";

let sentCommands: EmailDeliveryCommand[];
let sentEmails: EmailMessage[];

beforeEach(() => {
  sentCommands = [];
  sentEmails = [];
});

afterEach(() => {
  vi.useRealTimers();
});

describe("authenticated enqueue", () => {
  it("accepts a valid command without exposing the recipient in the queue body", async () => {
    const command = await commandFixture();
    const body = encoder.encode(JSON.stringify(command));
    const timestamp = Math.floor(Date.now() / 1000);
    const response = await worker.fetch(
      new Request("https://email.example/v1/enqueue", {
        method: "POST",
        body,
        headers: await signedHeaders(timestamp, body),
      }),
      environment(),
    );
    expect(response.status).toBe(202);
    expect(sentCommands).toEqual([command]);
    expect(JSON.stringify(sentCommands)).not.toContain("owner@example.com");
    expect(JSON.stringify(sentCommands)).not.toContain("12345678");
  });

  it("rejects unknown or legacy outer fields before enqueue", async () => {
    const command = await commandFixture();
    for (const extra of [
      { recipient: "owner@example.com" },
      { otp: "12345678" },
      { verification_url: "https://example.com/verify" },
      { unexpected: "secret" },
    ]) {
      const body = encoder.encode(JSON.stringify({ ...command, ...extra }));
      const response = await enqueueRequest(body, environment());
      expect(response.status).toBe(400);
    }
    expect(sentCommands).toHaveLength(0);
  });

  it("rejects tampering and stale signatures", async () => {
    const command = await commandFixture();
    const body = encoder.encode(JSON.stringify(command));
    const stale = Math.floor(Date.now() / 1000) - 301;
    expect(
      (
        await worker.fetch(
          new Request("https://email.example/v1/enqueue", {
            method: "POST",
            body,
            headers: await signedHeaders(stale, body),
          }),
          environment(),
        )
      ).status,
    ).toBe(401);
    expect(sentCommands).toHaveLength(0);
  });

  it("accepts exactly 8192 signed bytes", async () => {
    const command = await commandFixture();
    const json = encoder.encode(JSON.stringify(command));
    const body = new Uint8Array(8192).fill(0x20);
    body.set(json);
    const timestamp = Math.floor(Date.now() / 1000);
    const response = await worker.fetch(
      new Request("https://email.example/v1/enqueue", {
        method: "POST",
        body,
        headers: await signedHeaders(timestamp, body),
      }),
      environment(),
    );
    expect(response.status).toBe(202);
    expect(sentCommands).toEqual([command]);
  });

  it("rejects an oversized Content-Length before reading or authenticating", async () => {
    const cancelled = vi.fn();
    const pulled = vi.fn();
    const stream = new ReadableStream<Uint8Array>({
      cancel: cancelled,
      pull(controller) {
        pulled();
        controller.enqueue(new Uint8Array([1]));
      },
    });
    const response = await worker.fetch(
      streamRequest(stream, new Headers({ "content-length": "8193" })),
      environment(),
    );
    expect(response.status).toBe(413);
    expect(cancelled).toHaveBeenCalledOnce();
    expect(pulled).not.toHaveBeenCalled();
  });

  it("cancels a chunked body as soon as byte 8193 is observed", async () => {
    const cancelled = vi.fn();
    let chunk = 0;
    const stream = new ReadableStream<Uint8Array>({
      cancel: cancelled,
      pull(controller) {
        if (chunk === 0) {
          controller.enqueue(new Uint8Array(8192).fill(0x20));
        } else {
          controller.enqueue(new Uint8Array([0x20]));
        }
        chunk += 1;
      },
    });
    const response = await worker.fetch(
      streamRequest(stream),
      environment(),
    );
    expect(response.status).toBe(413);
    expect(cancelled).toHaveBeenCalledOnce();
    expect(chunk).toBe(2);
  });

  it("binds signatures to the method, path, and key ID", async () => {
    const command = await commandFixture();
    const body = encoder.encode(JSON.stringify(command));
    const timestamp = Math.floor(Date.now() / 1000);
    const headers = await signedHeaders(timestamp, body);
    headers.set("x-taskveil-key-id", "other-key");
    const response = await worker.fetch(
      new Request("https://email.example/v1/enqueue", {
        method: "POST",
        body,
        headers,
      }),
      environment(),
    );
    expect(response.status).toBe(401);
    expect(sentCommands).toHaveLength(0);
  });

  it("keeps the API-to-Worker signature conformance vector stable", async () => {
    const headers = await signedHeaders(1_785_024_000, encoder.encode("{}"));
    expect(headers.get("x-taskveil-signature")).toBe(
      "FKG_nYAP0pQJvfRPngo1Ewlg9t0sfyEf0hregqPoLSo",
    );
  });
});

describe("ingress delivery-id idempotency", () => {
  it("enqueues one command for 20 concurrent authenticated replays", async () => {
    const ledger = memoryLedgerNamespace();
    let releaseQueueSend = (): void => {};
    const queueSendBlocked = new Promise<void>((resolve) => {
      releaseQueueSend = resolve;
    });
    let markQueueSendStarted = (): void => {};
    const queueSendStarted = new Promise<void>((resolve) => {
      markQueueSendStarted = resolve;
    });
    const env = environment(false, ledger.namespace, async (queued) => {
      sentCommands.push(queued);
      markQueueSendStarted();
      await queueSendBlocked;
    });
    const command = await commandFixture();
    const body = encoder.encode(JSON.stringify(command));
    const timestamp = Math.floor(Date.now() / 1000);
    const headers = await signedHeaders(timestamp, body);
    const pendingResponses = Array.from({ length: 20 }, () =>
      worker.fetch(
        new Request("https://email.example/v1/enqueue", {
          method: "POST",
          body: body.slice(),
          headers: new Headers(headers),
        }),
        env,
      ),
    );
    await queueSendStarted;
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
    releaseQueueSend();
    const responses = await Promise.all(pendingResponses);
    expect(
      responses.every(
        (response) => response.status === 202 || response.status === 503,
      ),
    ).toBe(true);
    expect(sentCommands).toEqual([command]);
  });

  it("bounds Queue response-loss duplication by the ingress lease", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
    const ledger = memoryLedgerNamespace();
    let queueCalls = 0;
    const env = environment(false, ledger.namespace, async (command) => {
      sentCommands.push(command);
      queueCalls += 1;
      if (queueCalls === 1) throw new Error("response lost after enqueue");
    });
    const command = await commandFixture();
    command.not_after = new Date(Date.now() + 15 * 60_000).toISOString();
    const body = encoder.encode(JSON.stringify(command));

    expect((await enqueueRequest(body, env)).status).toBe(503);
    expect((await enqueueRequest(body, env)).status).toBe(503);
    expect(sentCommands).toHaveLength(1);

    vi.setSystemTime(new Date("2026-07-26T00:05:00.001Z"));
    expect((await enqueueRequest(body, env)).status).toBe(202);
    expect((await enqueueRequest(body, env)).status).toBe(202);
    expect(sentCommands).toHaveLength(2);

    const firstAck = vi.fn();
    const secondAck = vi.fn();
    await worker.queue(
      {
        messages: [
          { body: sentCommands[0], ack: firstAck, retry: vi.fn() },
          { body: sentCommands[1], ack: secondAck, retry: vi.fn() },
        ],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      env,
    );
    expect(firstAck).toHaveBeenCalledOnce();
    expect(secondAck).toHaveBeenCalledOnce();
    expect(sentEmails).toHaveLength(1);
  });

  it.each([
    ["delta-seconds", "120", "120"],
    [
      "HTTP-date",
      new Date(Date.parse("2026-07-26T00:00:00.000Z") + 120_000).toUTCString(),
      "120",
    ],
    ["zero delta-seconds", "0", "1"],
    ["malformed value", "next Tuesday", "300"],
  ])(
    "normalizes a leased Durable Object %s Retry-After",
    async (_kind, retryAfter, expected) => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
      const command = await commandFixture();
      command.not_after = new Date(Date.now() + 15 * 60_000).toISOString();
      const response = await enqueueRequest(
        encoder.encode(JSON.stringify(command)),
        environment(false, leasedLedgerNamespace(retryAfter)),
      );
      expect(response.status).toBe(503);
      expect(response.headers.get("retry-after")).toBe(expected);
      expect(sentCommands).toHaveLength(0);
    },
  );
});

describe("durable delivery ledger retention", () => {
  it.each(["sending", "accepted"] as const)(
    "retains %s delivery state until not_after plus Queue retention and margin",
    async (terminalState) => {
      vi.useFakeTimers();
      vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
      const ledger = memoryLedger();
      const notAfter = "2026-07-26T00:10:00.000Z";
      expect(
        (
          await ledger.object.fetch(
            claimLedgerRequest("/delivery/claim", notAfter),
          )
        ).status,
      ).toBe(201);
      if (terminalState === "accepted") {
        expect(
          (
            await ledger.object.fetch(
              ledgerRequest("/delivery/complete"),
            )
          ).status,
        ).toBe(204);
      }
      expect(ledger.storage.values.has("state")).toBe(true);
      expect(ledger.storage.alarmAt).toBe(
        Date.parse(notAfter) + 60 * 60 * 1000 + 5 * 60 * 1000,
      );

      await ledger.object.alarm();
      expect(ledger.storage.values.size).toBe(0);
    },
  );

  it("propagates alarm deletion failure so Cloudflare can retry it", async () => {
    const ledger = memoryLedger();
    const notAfter = new Date(Date.now() + 60_000).toISOString();
    await ledger.object.fetch(claimLedgerRequest("/ingress/claim", notAfter));
    ledger.storage.failNextDeleteAll = true;
    await expect(ledger.object.alarm()).rejects.toThrow("transient storage failure");
    expect(ledger.storage.values.has("state")).toBe(true);
    await ledger.object.alarm();
    expect(ledger.storage.values.size).toBe(0);
    expect(ledger.storage.deleteAllCalls).toBe(2);
  });
});

describe("queue delivery", () => {
  it("decrypts only at the email boundary and acknowledges Email Service acceptance", async () => {
    const ack = vi.fn();
    const retry = vi.fn();
    const command = await commandFixture();
    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(),
    );
    expect(ack).toHaveBeenCalledOnce();
    expect(retry).not.toHaveBeenCalled();
    expect(sentEmails).toHaveLength(1);
    expect(sentEmails[0]?.to).toBe("owner@example.com");
    const raw = (sentEmails[0] as unknown as { raw: string }).raw;
    expect(raw).toContain("\r\n12345678\r\n");
    expect(raw).toContain(
      "If you did not request this code, you can safely ignore this email.",
    );
    expect(raw).not.toContain("https://");
  });

  it("acks expired commands without attempting delivery", async () => {
    const ack = vi.fn();
    const retry = vi.fn();
    const command = await commandFixture();
    command.not_after = new Date(Date.now() - 1).toISOString();
    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(),
    );
    expect(ack).toHaveBeenCalledOnce();
    expect(sentEmails).toHaveLength(0);
  });

  it("retries when Email Service does not accept the message", async () => {
    const ack = vi.fn();
    const retry = vi.fn();
    const command = await commandFixture();
    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(true),
    );
    expect(ack).not.toHaveBeenCalled();
    expect(retry).toHaveBeenCalledOnce();
    expect(retry).toHaveBeenCalledWith({ delaySeconds: 300 });
  });

  it("rejects non-canonical or header-like recipient syntax at the boundary", async () => {
    const ack = vi.fn();
    const retry = vi.fn();
    const command = await commandFixture("display <owner@example.com>");
    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(),
    );
    expect(ack).toHaveBeenCalledOnce();
    expect(retry).not.toHaveBeenCalled();
    expect(sentEmails).toHaveLength(0);
  });

  it.each(["1234567", "123456789", "1234A678", "１２３４５６７８"])(
    "rejects a non-eight-ASCII-digit OTP: %s",
    async (otp) => {
      const ack = vi.fn();
      const retry = vi.fn();
      const command = await commandFixture("owner@example.com", otp);
      await worker.queue(
        {
          messages: [{ body: command, ack, retry }],
        } as unknown as MessageBatch<EmailDeliveryCommand>,
        environment(),
      );
      expect(ack).toHaveBeenCalledOnce();
      expect(retry).not.toHaveBeenCalled();
      expect(sentEmails).toHaveLength(0);
    },
  );

  it("rejects legacy link payloads and unknown secret-bearing fields", async () => {
    const ack = vi.fn();
    const retry = vi.fn();
    const command = await commandForPayload({
      recipient: "owner@example.com",
      verification_url: "https://api.example.com/verify-email#1.secret",
      template: "verify-email-v1",
      token: "secret",
    });
    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(),
    );
    expect(ack).toHaveBeenCalledOnce();
    expect(retry).not.toHaveBeenCalled();
    expect(sentEmails).toHaveLength(0);
  });

  it("defers a live delivery lease without consuming the Queue attempt budget", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
    const ledger = memoryLedgerNamespace();
    const command = await commandFixture();
    command.not_after = new Date(Date.now() + 15 * 60_000).toISOString();
    const object = ledger.objects.get(command.delivery_id) ?? memoryLedger();
    ledger.objects.set(command.delivery_id, object);
    expect(
      (
        await object.object.fetch(
          claimLedgerRequest("/delivery/claim", command.not_after),
        )
      ).status,
    ).toBe(201);
    const queueSend = vi.fn(
      async (
        queued: EmailDeliveryCommand,
        _options?: QueueSendOptions,
      ): Promise<void> => {
        sentCommands.push(queued);
      },
    );
    const ack = vi.fn();
    const retry = vi.fn();

    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(false, ledger.namespace, queueSend),
    );

    expect(queueSend).toHaveBeenCalledWith(command, {
      contentType: "json",
      delaySeconds: 300,
    });
    expect(ack).toHaveBeenCalledOnce();
    expect(retry).not.toHaveBeenCalled();
    expect(sentEmails).toHaveLength(0);
  });

  it("falls back to retry only if publishing a leased replacement fails", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
    const command = await commandFixture();
    command.not_after = new Date(Date.now() + 15 * 60_000).toISOString();
    const ack = vi.fn();
    const retry = vi.fn();
    const queueSend = vi.fn(async () => {
      throw new Error("Queue unavailable");
    });

    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(
        false,
        leasedLedgerNamespace(
          new Date(Date.now() + 90_000).toUTCString(),
        ),
        queueSend,
      ),
    );

    expect(queueSend).toHaveBeenCalledWith(command, {
      contentType: "json",
      delaySeconds: 90,
    });
    expect(ack).not.toHaveBeenCalled();
    expect(retry).toHaveBeenCalledWith({ delaySeconds: 90 });
  });

  it("acks rather than deferring a lease beyond command expiry", async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-07-26T00:00:00.000Z"));
    const command = await commandFixture();
    command.not_after = new Date(Date.now() + 60_000).toISOString();
    const ack = vi.fn();
    const retry = vi.fn();
    const queueSend = vi.fn();

    await worker.queue(
      {
        messages: [{ body: command, ack, retry }],
      } as unknown as MessageBatch<EmailDeliveryCommand>,
      environment(
        false,
        leasedLedgerNamespace(
          new Date(Date.now() + 90_000).toUTCString(),
        ),
        queueSend,
      ),
    );

    expect(queueSend).not.toHaveBeenCalled();
    expect(retry).not.toHaveBeenCalled();
    expect(ack).toHaveBeenCalledOnce();
  });
});

function environment(
  rejectEmail = false,
  ledger?: DurableObjectNamespace,
  queueSend?: (
    command: EmailDeliveryCommand,
    options?: QueueSendOptions,
  ) => Promise<void>,
): CloudflareBindings {
  const result: CloudflareBindings = {
    EMAIL_COMMANDS: {
      send:
        queueSend ??
        (async (command: EmailDeliveryCommand) => {
          sentCommands.push(command);
        }),
    } as unknown as Queue<EmailDeliveryCommand>,
    EMAIL_DELIVERY_LEDGER: ledger ?? ({} as DurableObjectNamespace),
    SEND_EMAIL: {
      send: async (message: EmailMessage) => {
        if (rejectEmail) throw new Error("test rejection");
        sentEmails.push(message);
        return { messageId: "test-message-id" };
      },
    } as unknown as SendEmail,
    EMAIL_FROM: "verify@example.com",
    DELIVERY_SIGNING_KEY_CURRENT_ID: "sign-v1",
    DELIVERY_SIGNING_KEY_CURRENT: base64(signingKey),
    DELIVERY_SIGNING_KEY_PREVIOUS_ID: "disabled",
    DELIVERY_SIGNING_KEY_PREVIOUS: "",
    DATA_KEY_CURRENT_VERSION: "1",
    DATA_KEY_CURRENT: base64(dataKey),
    DATA_KEY_PREVIOUS_VERSION: "0",
    DATA_KEY_PREVIOUS: "",
  };
  if (!ledger) result.TEST_ONLY_DISABLE_LEDGER = "enabled";
  return result;
}

async function commandFixture(
  recipient = "owner@example.com",
  otp = "12345678",
): Promise<EmailDeliveryCommand> {
  return commandForPayload({
    recipient,
    otp,
    template: "verify-email-otp-v1",
  });
}

async function commandForPayload(
  payloadValue: unknown,
): Promise<EmailDeliveryCommand> {
  const payload = encoder.encode(
    JSON.stringify(payloadValue),
  );
  const nonce = new Uint8Array(12).fill(9);
  const key = await crypto.subtle.importKey("raw", dataKey, "AES-GCM", false, [
    "encrypt",
  ]);
  const encrypted = new Uint8Array(
    await crypto.subtle.encrypt(
      {
        name: "AES-GCM",
        iv: nonce,
        additionalData: encryptionAad(1, deliveryBinding(deliveryId)),
      },
      key,
      payload,
    ),
  );
  const sealed = new Uint8Array(20 + encrypted.length);
  sealed.set(encoder.encode("TVE1"));
  new DataView(sealed.buffer).setUint32(4, 1);
  sealed.set(nonce, 8);
  sealed.set(encrypted, 20);
  return {
    version: 1,
    delivery_id: deliveryId,
    not_after: new Date(Date.now() + 60_000).toISOString(),
    encrypted_payload: base64Url(sealed),
  };
}

async function signedHeaders(
  timestamp: number,
  body: Uint8Array,
): Promise<Headers> {
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", body));
  const input = encoder.encode(
    `POST\n/v1/enqueue\nsign-v1\n${timestamp}\n${base64Url(digest)}`,
  );
  const key = await crypto.subtle.importKey(
    "raw",
    signingKey,
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const signature = new Uint8Array(await crypto.subtle.sign("HMAC", key, input));
  return new Headers({
    "content-type": "application/json",
    "x-taskveil-key-id": "sign-v1",
    "x-taskveil-timestamp": String(timestamp),
    "x-taskveil-signature": base64Url(signature),
  });
}

async function enqueueRequest(
  body: Uint8Array,
  env: CloudflareBindings,
): Promise<Response> {
  const timestamp = Math.floor(Date.now() / 1000);
  return worker.fetch(
    new Request("https://email.example/v1/enqueue", {
      body: body.slice(),
      headers: await signedHeaders(timestamp, body),
      method: "POST",
    }),
    env,
  );
}

function streamRequest(
  body: ReadableStream<Uint8Array>,
  headers = new Headers(),
): Request {
  return new Request(
    "https://email.example/v1/enqueue",
    {
      body,
      headers,
      method: "POST",
      duplex: "half",
    } as RequestInit & { duplex: "half" },
  );
}

interface MemoryStorage {
  alarmAt?: number;
  deleteAllCalls: number;
  failNextDeleteAll: boolean;
  values: Map<string, unknown>;
}

function memoryLedger(): {
  object: EmailDeliveryLedger;
  storage: MemoryStorage;
  stub: DurableObjectStub;
} {
  const storage: MemoryStorage = {
    deleteAllCalls: 0,
    failNextDeleteAll: false,
    values: new Map<string, unknown>(),
  };
  const durableStorage = {
    deleteAll: async () => {
      storage.deleteAllCalls += 1;
      if (storage.failNextDeleteAll) {
        storage.failNextDeleteAll = false;
        throw new Error("transient storage failure");
      }
      storage.values.clear();
      delete storage.alarmAt;
    },
    get: async <T>(key: string): Promise<T | undefined> =>
      storage.values.get(key) as T | undefined,
    put: async (key: string, value: unknown) => {
      storage.values.set(key, structuredClone(value));
    },
    setAlarm: async (scheduledTime: number | Date) => {
      storage.alarmAt =
        scheduledTime instanceof Date
          ? scheduledTime.getTime()
          : scheduledTime;
    },
  } as unknown as DurableObjectStorage;
  const object = new EmailDeliveryLedger(
    { storage: durableStorage } as DurableObjectState,
    {} as CloudflareBindings,
  );
  const stub = {
    fetch: async (
      input: RequestInfo | URL,
      init?: RequestInit<RequestInitCfProperties>,
    ) => object.fetch(new Request(input, init)),
  } as unknown as DurableObjectStub;
  return { object, storage, stub };
}

function memoryLedgerNamespace(): {
  namespace: DurableObjectNamespace;
  objects: Map<string, ReturnType<typeof memoryLedger>>;
} {
  const objects = new Map<string, ReturnType<typeof memoryLedger>>();
  const namespace = {
    jurisdiction: () => ({
      getByName: (name: string) => {
        let object = objects.get(name);
        if (!object) {
          object = memoryLedger();
          objects.set(name, object);
        }
        return object.stub;
      },
    }),
  } as unknown as DurableObjectNamespace;
  return { namespace, objects };
}

function leasedLedgerNamespace(retryAfter: string): DurableObjectNamespace {
  return {
    jurisdiction: () => ({
      getByName: () =>
        ({
          fetch: async () =>
            new Response(null, {
              headers: { "Retry-After": retryAfter },
              status: 409,
            }),
        }) as unknown as DurableObjectStub,
    }),
  } as unknown as DurableObjectNamespace;
}

function ledgerRequest(path: string): Request {
  return new Request(`https://delivery.internal${path}`, { method: "POST" });
}

function claimLedgerRequest(path: string, notAfter: string): Request {
  return new Request(`https://delivery.internal${path}`, {
    body: JSON.stringify({ not_after: notAfter }),
    headers: { "content-type": "application/json" },
    method: "POST",
  });
}

function deliveryBinding(value: string): Uint8Array {
  const [uuid, generation] = value.split(":");
  const hex = uuid!.replaceAll("-", "");
  const result = new Uint8Array(20);
  for (let index = 0; index < 16; index += 1) {
    result[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  new DataView(result.buffer).setInt32(16, Number(generation));
  return result;
}

function encryptionAad(version: number, binding: Uint8Array): Uint8Array {
  const prefix = encoder.encode("taskveil/email-verification/aead/v1\0");
  const purpose = encoder.encode("taskveil/email/delivery-payload/v1\0");
  const result = new Uint8Array(prefix.length + 4 + purpose.length + 1 + binding.length);
  result.set(prefix);
  new DataView(result.buffer).setUint32(prefix.length, version);
  result.set(purpose, prefix.length + 4);
  result.set(binding, prefix.length + 4 + purpose.length + 1);
  return result;
}

function base64(value: Uint8Array): string {
  return Buffer.from(value).toString("base64");
}

function base64Url(value: Uint8Array): string {
  return Buffer.from(value).toString("base64url");
}
