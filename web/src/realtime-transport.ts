const SERVICE_PATH = '/voice_elf.v1.ApiService';
const encoder = new TextEncoder();
const decoder = new TextDecoder();

export interface RealtimeTransport {
  readonly open: boolean;
  sendJson(value: object): void;
  sendAudio(value: ArrayBuffer): void;
  close(): void;
}

interface TransportCallbacks {
  message: (value: string | ArrayBuffer) => void;
  open: () => void;
  close: (error?: Error) => void;
}

export function connectWebSocket(roomId: string, callbacks: TransportCallbacks): RealtimeTransport {
  const scheme = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const socket = new WebSocket(
    `${scheme}//${window.location.host}/ws?room_id=${encodeURIComponent(roomId)}`,
  );
  socket.binaryType = 'arraybuffer';
  socket.onopen = callbacks.open;
  socket.onmessage = (event) => callbacks.message(event.data as string | ArrayBuffer);
  socket.onerror = () => callbacks.close(new Error('WebSocket 连接失败'));
  socket.onclose = () => callbacks.close();
  return {
    get open() {
      return socket.readyState === WebSocket.OPEN;
    },
    sendJson(value) {
      if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(value));
    },
    sendAudio(value) {
      if (socket.readyState === WebSocket.OPEN) socket.send(value);
    },
    close() {
      socket.onclose = null;
      socket.onerror = null;
      socket.close();
    },
  };
}

export function connectGrpcRealtime(
  roomId: string,
  callbacks: TransportCallbacks,
): RealtimeTransport {
  const abort = new AbortController();
  let sessionId = '';
  let connected = false;
  let closed = false;
  let queue = Promise.resolve();
  let pendingAudio: Uint8Array[] = [];
  let flushTimer = 0;

  const fail = (error?: unknown) => {
    if (closed) return;
    closed = true;
    abort.abort();
    window.clearTimeout(flushTimer);
    callbacks.close(error instanceof Error ? error : undefined);
  };
  const enqueue = (payload: Uint8Array) => {
    if (!sessionId || closed) return;
    queue = queue
      .then(() => unary('/SendRealtime', encodeRealtimeInput(sessionId, payload)))
      .catch(fail);
  };
  const flushAudio = () => {
    window.clearTimeout(flushTimer);
    flushTimer = 0;
    if (!pendingAudio.length) return;
    const audio = concat(pendingAudio);
    pendingAudio = [];
    enqueue(field(3, audio));
  };

  void serverStream(
    '/SubscribeRealtime',
    field(1, encoder.encode(roomId)),
    abort.signal,
    (message) => {
      const output = decodeRealtimeOutput(message);
      sessionId ||= output.sessionId;
      if (!connected) {
        connected = true;
        callbacks.open();
      }
      if (output.eventJson !== null) callbacks.message(output.eventJson);
      else if (output.audio) {
        callbacks.message(
          output.audio.buffer.slice(
            output.audio.byteOffset,
            output.audio.byteOffset + output.audio.byteLength,
          ) as ArrayBuffer,
        );
      }
    },
  ).then(() => fail(), fail);

  return {
    get open() {
      return connected && !closed && Boolean(sessionId);
    },
    sendJson(value) {
      flushAudio();
      enqueue(field(2, encoder.encode(JSON.stringify(value))));
    },
    sendAudio(value) {
      if (!sessionId || closed) return;
      pendingAudio.push(new Uint8Array(value));
      const bytes = pendingAudio.reduce((total, chunk) => total + chunk.byteLength, 0);
      if (bytes >= 8 * 1024) flushAudio();
      else if (!flushTimer) flushTimer = window.setTimeout(flushAudio, 64);
    },
    close() {
      closed = true;
      window.clearTimeout(flushTimer);
      abort.abort();
    },
  };
}

async function unary(method: string, message: Uint8Array) {
  const response = await fetch(`${SERVICE_PATH}${method}`, grpcRequest(message));
  if (!response.ok) throw new Error(`gRPC-Web 传输失败 (${response.status})`);
  const frames = framesFromBytes(new Uint8Array(await response.arrayBuffer()));
  assertTrailers(frames);
}

async function serverStream(
  method: string,
  message: Uint8Array,
  signal: AbortSignal,
  onMessage: (message: Uint8Array) => void,
) {
  const response = await fetch(`${SERVICE_PATH}${method}`, { ...grpcRequest(message), signal });
  if (!response.ok || !response.body) throw new Error(`gRPC-Web 流连接失败 (${response.status})`);
  const reader = response.body.getReader();
  let buffered = new Uint8Array();
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffered = concat([buffered, value]);
    const parsed = readCompleteFrames(buffered);
    buffered = parsed.remaining;
    for (const frame of parsed.frames) {
      if (frame.trailer) assertTrailer(frame.payload);
      else onMessage(frame.payload);
    }
  }
  if (buffered.length) throw new Error('gRPC-Web 流响应不完整');
}

function grpcRequest(message: Uint8Array): RequestInit {
  const framed = new Uint8Array(message.length + 5);
  new DataView(framed.buffer).setUint32(1, message.length, false);
  framed.set(message, 5);
  return {
    method: 'POST',
    credentials: 'include',
    cache: 'no-store',
    headers: {
      accept: 'application/grpc-web+proto',
      'content-type': 'application/grpc-web+proto',
      'x-grpc-web': '1',
    },
    body: framed,
  };
}

interface Frame {
  trailer: boolean;
  payload: Uint8Array;
}

function framesFromBytes(bytes: Uint8Array) {
  const parsed = readCompleteFrames(bytes);
  if (parsed.remaining.length) throw new Error('gRPC-Web 响应不完整');
  return parsed.frames;
}

function readCompleteFrames(input: Uint8Array) {
  const frames: Frame[] = [];
  let offset = 0;
  while (offset + 5 <= input.length) {
    const flags = input[offset];
    const length = new DataView(input.buffer, input.byteOffset + offset + 1, 4).getUint32(0, false);
    if (offset + 5 + length > input.length) break;
    frames.push({ trailer: (flags & 0x80) !== 0, payload: input.subarray(offset + 5, offset + 5 + length) });
    offset += 5 + length;
  }
  return { frames, remaining: input.slice(offset) };
}

function assertTrailers(frames: Frame[]) {
  const trailer = frames.find((frame) => frame.trailer);
  if (trailer) assertTrailer(trailer.payload);
}

function assertTrailer(payload: Uint8Array) {
  const values = new Map<string, string>();
  for (const line of decoder.decode(payload).split('\r\n')) {
    const separator = line.indexOf(':');
    if (separator > 0) values.set(line.slice(0, separator).toLowerCase(), line.slice(separator + 1).trim());
  }
  const status = Number(values.get('grpc-status') ?? '0');
  if (status !== 0) throw new Error(decodeURIComponent(values.get('grpc-message') ?? `gRPC ${status}`));
}

function encodeRealtimeInput(sessionId: string, payload: Uint8Array) {
  return concat([field(1, encoder.encode(sessionId)), payload]);
}

function decodeRealtimeOutput(message: Uint8Array) {
  const reader = new Reader(message);
  let sessionId = '';
  let eventJson: string | null = null;
  let audio: Uint8Array | null = null;
  while (!reader.done) {
    const tag = reader.varint();
    const fieldNumber = tag >>> 3;
    const wire = tag & 7;
    if (fieldNumber === 1 && wire === 2) sessionId = decoder.decode(reader.bytes());
    else if (fieldNumber === 2 && wire === 2) eventJson = decoder.decode(reader.bytes());
    else if (fieldNumber === 3 && wire === 2) audio = reader.bytes();
    else reader.skip(wire);
  }
  return { sessionId, eventJson, audio };
}

function field(number: number, value: Uint8Array) {
  return concat([varint((number << 3) | 2), varint(value.length), value]);
}

function varint(value: number) {
  const bytes: number[] = [];
  let remaining = value >>> 0;
  do {
    let byte = remaining & 0x7f;
    remaining >>>= 7;
    if (remaining) byte |= 0x80;
    bytes.push(byte);
  } while (remaining);
  return Uint8Array.from(bytes);
}

function concat(chunks: Uint8Array[]) {
  const result = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    result.set(chunk, offset);
    offset += chunk.length;
  }
  return result;
}

class Reader {
  private offset = 0;
  constructor(private readonly value: Uint8Array) {}
  get done() { return this.offset >= this.value.length; }
  varint() {
    let result = 0;
    for (let shift = 0; shift < 35; shift += 7) {
      const byte = this.value[this.offset++];
      result |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return result >>> 0;
    }
    throw new Error('Protobuf varint 无效');
  }
  bytes() {
    const end = this.offset + this.varint();
    if (end > this.value.length) throw new Error('Protobuf 字段不完整');
    const result = this.value.subarray(this.offset, end);
    this.offset = end;
    return result;
  }
  skip(wire: number) {
    if (wire === 0) this.varint();
    else if (wire === 1) this.offset += 8;
    else if (wire === 2) this.offset += this.varint();
    else if (wire === 5) this.offset += 4;
    else throw new Error(`不支持的 Protobuf wire type: ${wire}`);
  }
}
