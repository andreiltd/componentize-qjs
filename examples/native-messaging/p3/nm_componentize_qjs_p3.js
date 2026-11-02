//! componentize-qjs Native Messaging host
//! guest271314, andreiltd (https://github.com/andreiltd), 6-23-2026
//! https://github.com/andreiltd/issues/1
//! Based on https://github.com/guest271314/native-messaging-webassembly/nm_javy.js
//! WASI P3

import stdin from "wasi:cli/stdin@0.3.0";
import stdout from "wasi:cli/stdout@0.3.0";

const MAX_IN = 64 * 1024 * 1024;
const FRAME = 1024 * 1024;
const COMMA = 0x2c, OPEN = 0x5b, CLOSE = 0x5d;

async function readExact(input, n) {
  const out = new Uint8Array(n);
  let off = 0;
  while (off < n) {
    const chunk = await input.read(n - off);
    if (chunk.length === 0) {
      if (off === 0) return null;
      throw new Error(`stdin closed after ${off} of ${n} bytes`);
    }
    out.set(chunk, off);
    off += chunk.length;
  }
  return out;
}

async function readMessage(input) {
  const header = await readExact(input, 4);
  if (header === null) return null;
  const len = new DataView(header.buffer).getUint32(0, true);
  if (len > MAX_IN) throw new Error(`message of ${len} bytes exceeds 64 MiB`);
  return len === 0 ? new Uint8Array(0) : readExact(input, len);
}

async function writeFrame(output, body) {
  const frame = new Uint8Array(4 + body.length);
  new DataView(frame.buffer).setUint32(0, body.length, true);
  frame.set(body, 4);
  await output.writeAll(frame);
}

async function sendMessage(output, msg) {
  if (msg.length <= FRAME) {
    await writeFrame(output, msg);
    return;
  }
  for (let i = 1, end = msg.length - 1; i < end;) {
    let j = i + FRAME - 16;
    if (j >= end) j = end;
    else {
      const c = msg.indexOf(COMMA, j);
      j = c === -1 ? end : c;
    }
    const body = new Uint8Array(2 + (j - i));
    body[0] = OPEN;
    body.set(msg.subarray(i, j), 1);
    body[body.length - 1] = CLOSE;
    await writeFrame(output, body);
    i = j + 1;
  }
}

export const run = {
  async run() {
    const [input, status] = stdin.readViaStream();
    const { readable, writable } = wit.Stream(wit.Stream.U8);
    // Pipe framed output to stdout; await the future only after dropping the
    // writable end below.
    const writeDone = stdout.writeViaStream(readable);

    try {
      for (let msg; (msg = await readMessage(input)) !== null;) {
        await sendMessage(writable, msg);
      }
    } finally {
      writable.drop();
      input.drop();
    }

    const written = await writeDone;
    if (written.tag === "err") throw written.val;
    const statusResult = await status.read();
    if (statusResult.tag === "err") throw statusResult.val;
  },
};
