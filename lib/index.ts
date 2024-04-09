import { createRequire } from "node:module";
import { lookup } from "node:dns/promises";

const require = createRequire(import.meta.url);

const lib = require("./lib.node");

export type ConnectOptions = {
  hostname: string;
  port: number;
  onClose: (reason: string) => void;
};

export const connect = async (options: ConnectOptions): Promise<Connection> => {
  const address = await lookup(options.hostname);

  const connection = await lib.connect(
    options.port,
    address.address,
    options.hostname,
    options.onClose
  );

  return new Connection(connection);
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
