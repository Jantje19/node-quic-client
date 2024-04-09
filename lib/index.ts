import { createRequire } from "node:module";
import { lookup } from "node:dns/promises";

const require = createRequire(import.meta.url);

const lib = require("./lib.node");

export type ConnectOptions = {
  hostname: string;
  port: number;
  onClose: (reason: string) => void;
  alpnProtocols?: string[];
  certificateAuthorities?: Uint8Array[];
};

export const rawConnect = async (
  options: ConnectOptions & { ipAddress: string }
) => {
  let alpnProtocols;

  if (Array.isArray(options.alpnProtocols)) {
    const textEncoder = new TextEncoder();

    alpnProtocols = options.alpnProtocols.map((protocol) =>
      textEncoder.encode(protocol)
    );
  }

  const connection = await lib.connect(
    options.port,
    options.ipAddress,
    options.hostname,
    options.onClose,
    alpnProtocols,
    options.certificateAuthorities
  );

  return new Connection(connection);
};

export const connect = async (options: ConnectOptions): Promise<Connection> => {
  const address = await lookup(options.hostname);

  return rawConnect({ ...options, ipAddress: address.address });
};

export type StreamOptions = {
  onData: (packet: Uint8Array) => void;
  onClose: (reason: string) => void;
  onError: (error: Error) => void;
};

class Connection {
  private connection: unknown;

  constructor(connection: undefined) {
    this.connection = connection;
  }

  async createStream(options: StreamOptions): Promise<Stream> {
    const stream = await lib.create_stream(
      this.connection,
      options.onData,
      options.onClose,
      options.onError
    );

    return new Stream(this.connection, stream);
  }

  async close(errorCode?: number, reason?: string) {
    await lib.close_connection(
      this.connection,
      errorCode ?? 0,
      new TextEncoder().encode(reason ?? "")
    );
  }
}

class Stream {
  // Not used but necessary to prevent garbage collection
  private connection: unknown;
  private stream: unknown;

  constructor(connection: unknown, stream: unknown) {
    this.connection = connection;
    this.stream = stream;
  }

  async write(packet: Buffer): Promise<number> {
    return lib.write(this.stream, packet);
  }

  async close() {
    await lib.close_stream(this.stream);
  }

  async finish() {
    await lib.close_write(this.stream);
  }
}
