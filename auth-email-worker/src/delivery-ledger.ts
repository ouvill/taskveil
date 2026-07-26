import { DurableObject } from "cloudflare:workers";

interface LeaseState {
  status: "enqueueing" | "enqueued" | "sending" | "accepted";
  leaseUntil?: number;
}

interface DeliveryState {
  deleteAt: number;
  ingress?: LeaseState;
  delivery?: LeaseState;
}

interface ClaimBody {
  not_after: string;
}

const LEASE_MILLISECONDS = 5 * 60 * 1000;
const QUEUE_RETENTION_MILLISECONDS = 60 * 60 * 1000;
const RETENTION_MARGIN_MILLISECONDS = 5 * 60 * 1000;

export class EmailDeliveryLedger extends DurableObject<CloudflareBindings> {
  private tail: Promise<void> = Promise.resolve();

  override async fetch(request: Request): Promise<Response> {
    if (request.method !== "POST") return new Response(null, { status: 405 });
    const path = new URL(request.url).pathname;
    return this.exclusive(async () => {
      if (path === "/ingress/claim") {
        return this.claim("ingress", request);
      }
      if (path === "/ingress/complete") {
        return this.complete("ingress");
      }
      if (path === "/delivery/claim") {
        return this.claim("delivery", request);
      }
      if (path === "/delivery/complete") {
        return this.complete("delivery");
      }
      return new Response(null, { status: 404 });
    });
  }

  override async alarm(): Promise<void> {
    await this.exclusive(async () => {
      // Let Cloudflare retry the alarm if deletion fails. Retaining an
      // idempotency record longer is safe; losing it early is not.
      await this.ctx.storage.deleteAll();
    });
  }

  private async claim(
    kind: "ingress" | "delivery",
    request: Request,
  ): Promise<Response> {
    const notAfter = await parseClaim(request);
    if (notAfter === undefined) return new Response(null, { status: 400 });

    const deleteAt =
      notAfter + QUEUE_RETENTION_MILLISECONDS + RETENTION_MARGIN_MILLISECONDS;
    const state = await this.ctx.storage.get<DeliveryState>("state");
    if (state && state.deleteAt !== deleteAt) {
      // A delivery ID identifies one immutable command. Reusing it with a
      // different expiry is a terminal protocol violation.
      return new Response(null, { status: 422 });
    }

    const current = state?.[kind];
    const completedStatus = kind === "ingress" ? "enqueued" : "accepted";
    const activeStatus = kind === "ingress" ? "enqueueing" : "sending";
    if (current?.status === completedStatus) {
      return new Response(null, { status: 200 });
    }
    if (
      current?.status === activeStatus &&
      (current.leaseUntil ?? 0) > Date.now()
    ) {
      return new Response(null, {
        headers: { "Retry-After": leaseRetryAfter(current.leaseUntil!) },
        status: 409,
      });
    }

    const next: DeliveryState = state ?? { deleteAt };
    next[kind] = {
      status: activeStatus,
      leaseUntil: Date.now() + LEASE_MILLISECONDS,
    };
    await this.ctx.storage.setAlarm(deleteAt);
    await this.ctx.storage.put("state", next);
    return new Response(null, { status: 201 });
  }

  private async complete(kind: "ingress" | "delivery"): Promise<Response> {
    const state = await this.ctx.storage.get<DeliveryState>("state");
    const expected = kind === "ingress" ? "enqueueing" : "sending";
    if (state?.[kind]?.status !== expected) {
      return new Response(null, { status: 409 });
    }
    state[kind] = {
      status: kind === "ingress" ? "enqueued" : "accepted",
    };
    await this.ctx.storage.put("state", state);
    return new Response(null, { status: 204 });
  }

  private async exclusive<T>(operation: () => Promise<T>): Promise<T> {
    const previous = this.tail;
    let release = (): void => {};
    this.tail = new Promise<void>((resolve) => {
      release = resolve;
    });
    await previous;
    try {
      return await operation();
    } finally {
      release();
    }
  }
}

async function parseClaim(request: Request): Promise<number | undefined> {
  let candidate: unknown;
  try {
    candidate = await request.json();
  } catch {
    return undefined;
  }
  if (
    typeof candidate !== "object" ||
    candidate === null ||
    typeof (candidate as Partial<ClaimBody>).not_after !== "string"
  ) {
    return undefined;
  }
  const value = Date.parse((candidate as ClaimBody).not_after);
  return Number.isFinite(value) ? value : undefined;
}

function leaseRetryAfter(leaseUntil: number): string {
  return String(Math.max(1, Math.ceil((leaseUntil - Date.now()) / 1000)));
}
