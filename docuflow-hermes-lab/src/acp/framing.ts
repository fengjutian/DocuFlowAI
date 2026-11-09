/**
 * ACP framing.
 *
 * ACP over stdio is newline-delimited JSON, but LSP-style `Content-Length` framing is common enough
 * that a client may negotiate it. The reader detects the framing from the first frame; the writer
 * answers in the same framing so a client never has to handle two formats from one server.
 */
export type AcpFraming = "ndjson" | "content-length";

export class AcpFrameReader {
  private buffer: Buffer = Buffer.alloc(0);
  private detectedFraming: AcpFraming | undefined;

  constructor(private readonly mode: "auto" | AcpFraming = "auto") {
    if (mode !== "auto") this.detectedFraming = mode;
  }

  get framing(): AcpFraming | undefined {
    return this.detectedFraming;
  }

  push(chunk: Buffer | string): string[] {
    this.buffer = Buffer.concat([
      this.buffer,
      Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk, "utf8")
    ]);
    const frames: string[] = [];

    while (true) {
      if (!this.detectedFraming) this.detectedFraming = this.detect();
      if (!this.detectedFraming) break;

      const frame = this.detectedFraming === "content-length"
        ? this.readContentLengthFrame()
        : this.readLineFrame();
      if (frame === undefined) break;
      if (frame !== "") frames.push(frame);
    }
    return frames;
  }

  private detect(): AcpFraming | undefined {
    const preview = this.buffer.subarray(0, Math.min(this.buffer.length, 32)).toString("utf8");
    if (/^\s*content-length\s*:/i.test(preview)) return "content-length";
    if (!preview.trimStart().startsWith("{")) {
      // Neither a header nor JSON yet: wait for more bytes.
      if (!this.buffer.includes(0x0a)) return undefined;
    }
    return "ndjson";
  }

  private readLineFrame(): string | undefined {
    const newline = this.buffer.indexOf(0x0a);
    if (newline < 0) return undefined;
    const line = this.buffer.subarray(0, newline).toString("utf8").trim();
    this.buffer = this.buffer.subarray(newline + 1);
    return line;
  }

  private readContentLengthFrame(): string | undefined {
    const headerEnd = this.findHeaderEnd();
    if (typeof headerEnd === "number") return undefined;
    const header = this.buffer.subarray(0, headerEnd.index).toString("utf8");
    const match = /content-length\s*:\s*(\d+)/i.exec(header);
    if (!match) {
      // A malformed header block cannot be framed; drop it so the connection is not stuck forever.
      this.buffer = this.buffer.subarray(headerEnd.end);
      return "";
    }
    const length = Number(match[1]);
    const bodyStart = headerEnd.end;
    if (this.buffer.length < bodyStart + length) return undefined;
    const body = this.buffer.subarray(bodyStart, bodyStart + length).toString("utf8");
    this.buffer = this.buffer.subarray(bodyStart + length);
    return body;
  }

  private findHeaderEnd(): { index: number; end: number } | -1 {
    const crlf = this.buffer.indexOf("\r\n\r\n");
    if (crlf >= 0) return { index: crlf, end: crlf + 4 };
    const lf = this.buffer.indexOf("\n\n");
    if (lf >= 0) return { index: lf, end: lf + 2 };
    return -1;
  }
}

export function serializeFrame(message: unknown, framing: AcpFraming): string {
  const json = JSON.stringify(message);
  if (framing === "ndjson") return `${json}\n`;
  return `Content-Length: ${Buffer.byteLength(json, "utf8")}\r\n\r\n${json}`;
}
