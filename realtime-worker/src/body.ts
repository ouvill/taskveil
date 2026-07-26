import { MAX_PUBLISH_BODY_BYTES } from "./contracts";

const MAX_PUBLISH_BODY_PROBE_BYTES = MAX_PUBLISH_BODY_BYTES + 1;
const INVALID_BODY_REASON = "invalid request body";

/**
 * Reads a publish request body without trusting Content-Length.
 *
 * Header failures are rejected before the stream is read. A valid header permits
 * at most one byte beyond the application limit to be requested from the
 * byte-oriented Fetch body stream, which is enough to distinguish 512 bytes from
 * an oversized body. Every rejection cancels the remaining stream.
 */
export async function readPublishBody(request: Request): Promise<Uint8Array | null> {
  const expectedLength = canonicalContentLength(request.headers.get("Content-Length"));
  if (expectedLength === null || expectedLength > MAX_PUBLISH_BODY_BYTES) {
    await cancelStream(request.body);
    return null;
  }

  if (request.body === null) {
    return expectedLength === 0 ? new Uint8Array() : null;
  }

  let reader: ReadableStreamBYOBReader;
  try {
    reader = request.body.getReader({ mode: "byob" });
  } catch {
    await cancelStream(request.body);
    return null;
  }

  const chunks: Uint8Array[] = [];
  let bytesRead = 0;
  try {
    while (bytesRead <= MAX_PUBLISH_BODY_BYTES) {
      const remainingProbeBytes = MAX_PUBLISH_BODY_PROBE_BYTES - bytesRead;
      const result = await reader.read(new Uint8Array(remainingProbeBytes));
      const chunk = result.value;
      if (chunk && chunk.byteLength > 0) {
        chunks.push(chunk);
        bytesRead += chunk.byteLength;
        if (
          bytesRead > expectedLength ||
          bytesRead > MAX_PUBLISH_BODY_BYTES
        ) {
          await cancelReader(reader);
          return null;
        }
      }
      if (result.done) break;
    }

    if (bytesRead !== expectedLength) {
      await cancelReader(reader);
      return null;
    }
    return concatenate(chunks, bytesRead);
  } catch {
    await cancelReader(reader);
    return null;
  } finally {
    reader.releaseLock();
  }
}

function canonicalContentLength(value: string | null): number | null {
  if (value === null || !/^(0|[1-9][0-9]*)$/.test(value)) return null;
  const parsed = Number(value);
  return Number.isSafeInteger(parsed) ? parsed : null;
}

async function cancelReader(reader: ReadableStreamBYOBReader): Promise<void> {
  try {
    await reader.cancel(INVALID_BODY_REASON);
  } catch {
    // Rejection is already fail-closed; cancellation is best effort after a stream error.
  }
}

async function cancelStream(stream: ReadableStream<Uint8Array> | null): Promise<void> {
  if (stream === null) return;
  try {
    await stream.cancel(INVALID_BODY_REASON);
  } catch {
    // Rejection is already fail-closed; cancellation is best effort after a stream error.
  }
}

function concatenate(chunks: Uint8Array[], length: number): Uint8Array {
  const body = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}
