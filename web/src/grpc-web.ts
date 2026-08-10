const API_RPC_PATH = '/voice_elf.v1.ApiService/Call';
const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();

export interface GrpcApiResponse {
  status: number;
  headers: Headers;
  body: Uint8Array;
}

interface RpcHeader {
  name: string;
  value: Uint8Array;
}

export async function grpcApiCall(path: string, init: RequestInit = {}): Promise<GrpcApiResponse> {
  const headers = new Headers(init.headers);
  if (init.body && !(init.body instanceof FormData) && !headers.has('content-type')) {
    headers.set('content-type', 'application/json');
  }
  const request = new Request(new URL(path, window.location.href), { ...init, headers });
  const body = init.body ? new Uint8Array(await request.arrayBuffer()) : new Uint8Array();
  const message = encodeApiRequest(
    request.method,
    `${new URL(path, window.location.href).pathname}${new URL(path, window.location.href).search}`,
    [...request.headers].map(([name, value]) => ({ name, value: textEncoder.encode(value) })),
    body,
  );
  const framed = new Uint8Array(message.length + 5);
  new DataView(framed.buffer).setUint32(1, message.length, false);
  framed.set(message, 5);

  const response = await fetch(API_RPC_PATH, {
    method: 'POST',
    credentials: 'include',
    cache: 'no-store',
    signal: init.signal,
    headers: {
      accept: 'application/grpc-web+proto',
      'content-type': 'application/grpc-web+proto',
      'x-grpc-web': '1',
      'x-user-agent': 'voice-elf-web/0.1',
    },
    body: framed,
  });
  if (!response.ok) throw new Error(`gRPC-Web transport failed (${response.status})`);
  return decodeGrpcResponse(new Uint8Array(await response.arrayBuffer()));
}

function encodeApiRequest(method: string, path: string, headers: RpcHeader[], body: Uint8Array) {
  const fields = [
    encodeBytesField(1, textEncoder.encode(method)),
    encodeBytesField(2, textEncoder.encode(path)),
  ];
  for (const header of headers) {
    fields.push(
      encodeBytesField(
        3,
        concatBytes([
          encodeBytesField(1, textEncoder.encode(header.name)),
          encodeBytesField(2, header.value),
        ]),
      ),
    );
  }
  if (body.length) fields.push(encodeBytesField(4, body));
  return concatBytes(fields);
}

function encodeBytesField(field: number, value: Uint8Array) {
  return concatBytes([encodeVarint((field << 3) | 2), encodeVarint(value.length), value]);
}

function encodeVarint(value: number) {
  const output: number[] = [];
  let remaining = value >>> 0;
  do {
    let byte = remaining & 0x7f;
    remaining >>>= 7;
    if (remaining) byte |= 0x80;
    output.push(byte);
  } while (remaining);
  return Uint8Array.from(output);
}

function concatBytes(chunks: Uint8Array[]) {
  const output = new Uint8Array(chunks.reduce((total, chunk) => total + chunk.length, 0));
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}

function decodeGrpcResponse(bytes: Uint8Array): GrpcApiResponse {
  let offset = 0;
  let message: Uint8Array | null = null;
  let grpcStatus = 0;
  let grpcMessage = '';
  while (offset + 5 <= bytes.length) {
    const flags = bytes[offset];
    const length = new DataView(bytes.buffer, bytes.byteOffset + offset + 1, 4).getUint32(0, false);
    offset += 5;
    if (offset + length > bytes.length) throw new Error('gRPC-Web response frame is incomplete');
    const payload = bytes.subarray(offset, offset + length);
    offset += length;
    if ((flags & 0x80) !== 0) {
      const trailers = textDecoder.decode(payload);
      for (const line of trailers.split('\r\n')) {
        const separator = line.indexOf(':');
        if (separator < 0) continue;
        const name = line.slice(0, separator).trim().toLowerCase();
        const value = line.slice(separator + 1).trim();
        if (name === 'grpc-status') grpcStatus = Number(value);
        if (name === 'grpc-message') grpcMessage = decodeURIComponent(value);
      }
    } else if ((flags & 1) === 0) {
      message = payload;
    }
  }
  if (grpcStatus !== 0) throw new Error(grpcMessage || `gRPC request failed (${grpcStatus})`);
  if (!message) throw new Error('gRPC-Web response did not contain a message');
  return decodeApiResponse(message);
}

function decodeApiResponse(bytes: Uint8Array): GrpcApiResponse {
  const reader = new ProtoReader(bytes);
  let status = 0;
  const headers = new Headers();
  let body: Uint8Array<ArrayBufferLike> = new Uint8Array();
  while (!reader.done) {
    const tag = reader.varint();
    const field = tag >>> 3;
    const wire = tag & 7;
    if (field === 1 && wire === 0) status = reader.varint();
    else if (field === 2 && wire === 2) {
      const header = decodeHeader(reader.bytes());
      if (header) headers.append(header.name, textDecoder.decode(header.value));
    } else if (field === 3 && wire === 2) body = reader.bytes();
    else reader.skip(wire);
  }
  if (!status) throw new Error('gRPC API response did not include an HTTP status');
  return { status, headers, body };
}

function decodeHeader(bytes: Uint8Array): RpcHeader | null {
  const reader = new ProtoReader(bytes);
  let name = '';
  let value: Uint8Array<ArrayBufferLike> = new Uint8Array();
  while (!reader.done) {
    const tag = reader.varint();
    const field = tag >>> 3;
    const wire = tag & 7;
    if (field === 1 && wire === 2) name = textDecoder.decode(reader.bytes());
    else if (field === 2 && wire === 2) value = reader.bytes();
    else reader.skip(wire);
  }
  return name ? { name, value } : null;
}

class ProtoReader {
  private offset = 0;

  constructor(private readonly input: Uint8Array) {}

  get done() {
    return this.offset >= this.input.length;
  }

  varint() {
    let value = 0;
    let shift = 0;
    while (this.offset < this.input.length && shift < 35) {
      const byte = this.input[this.offset++];
      value |= (byte & 0x7f) << shift;
      if ((byte & 0x80) === 0) return value >>> 0;
      shift += 7;
    }
    throw new Error('Invalid protobuf varint');
  }

  bytes() {
    const length = this.varint();
    const end = this.offset + length;
    if (end > this.input.length) throw new Error('Invalid protobuf byte field');
    const value = this.input.subarray(this.offset, end);
    this.offset = end;
    return value;
  }

  skip(wire: number) {
    if (wire === 0) this.varint();
    else if (wire === 1) this.offset += 8;
    else if (wire === 2) this.offset += this.varint();
    else if (wire === 5) this.offset += 4;
    else throw new Error(`Unsupported protobuf wire type ${wire}`);
    if (this.offset > this.input.length) throw new Error('Invalid protobuf field length');
  }
}
