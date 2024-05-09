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
  clientAuthentication?: {
    certificate: Buffer;
    key: Buffer;
  };
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

  let clientAuthentication;

  if (
    options.clientAuthentication?.certificate &&
    options.clientAuthentication?.key
  ) {
    clientAuthentication = [
      options.clientAuthentication.certificate,
      options.clientAuthentication.key,
    ];
  }

  const connection = await lib.connect(
    options.port,
    options.ipAddress,
    options.hostname,
    options.onClose,
    alpnProtocols,
    options.certificateAuthorities,
    clientAuthentication
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

    return new Stream(this, stream);
  }

  async close(errorCode?: number, reason?: string) {
    const fullReason = reason ?? "";
    const buffer =
      fullReason.length > 0 ? new TextEncoder().encode(fullReason) : null;

    await lib.close_connection(this.connection, errorCode ?? 0, buffer);
  }

  getRemoteIp() {
    return lib.get_remote(this.connection);
  }
}

class Stream {
  private connection: Connection;
  private stream: unknown;

  constructor(connection: Connection, stream: unknown) {
    this.connection = connection;
    this.stream = stream;
  }

  async write(packet: Uint8Array): Promise<void> {
    if (packet.length > 0) {
      await lib.write_stream(this.stream, packet);
    }
  }

  async close() {
    await lib.close_stream(this.stream);
  }

  async finish() {
    await lib.close_write(this.stream);
  }

  getConnection() {
    return this.connection;
  }
}
