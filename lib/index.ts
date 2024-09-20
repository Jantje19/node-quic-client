import { createRequire } from "node:module";

const require = createRequire(import.meta.url);

const lib = require("./lib.node");

export type ConnectOptions = {
  hostname: string;
  port: number;
  onClose: (this: Connection, reason: string) => void;
  onError: (this: Connection, error: Error) => void;
  onStream: (this: Connection, partialStream: PartialStream) => void;
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

  const handleNewStream = (
    rawPartialStream: unknown,
    isUnidirectional: boolean
  ) => {
    const partialStream = new PartialStream(
      fullConnection,
      rawPartialStream,
      isUnidirectional
    );

    options.onStream.call(fullConnection, partialStream);

    if (!partialStream.isInitialized) {
      partialStream
        .initialize({ onClose: () => {}, onData: () => {}, onError: () => {} })
        .close()
        .catch(() => {});

      throw new Error("Partial stream has not been initialized");
    }
  };

  const connection = await lib.connect(
    options.port,
    options.ipAddress,
    options.hostname,
    handleNewStream,
    (...args: Parameters<ConnectOptions["onClose"]>) =>
      options.onClose.apply(fullConnection, args),
    (...args: Parameters<ConnectOptions["onError"]>) =>
      options.onError.apply(fullConnection, args),
    alpnProtocols,
    options.certificateAuthorities,
    clientAuthentication
  );

  const fullConnection = new Connection(connection);

  return fullConnection;
};

export const connect = async (options: ConnectOptions): Promise<Connection> => {
  const { lookup } = await import("node:dns/promises");

  const address = await lookup(options.hostname);

  return rawConnect({ ...options, ipAddress: address.address });
};

export type StreamOptions = {
  onData: (this: Stream, packet: Uint8Array) => void;
  onClose: (this: Stream, reason: string) => void;
  onError: (this: Stream, error: Error) => void;
};

export class Connection {
  private connection: unknown;

  constructor(connection: undefined) {
    this.connection = connection;
  }

  async createStream(options: StreamOptions): Promise<Stream> {
    const partialStream = await lib.create_stream(this.connection);

    return new PartialStream(this, partialStream, false).initialize(options);
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

export class PartialStream {
  private connection: Connection;
  private partialStream: unknown;
  private writeClosed: boolean;
  private initialized = false;

  get isInitialized() {
    return this.initialized;
  }

  constructor(
    connection: Connection,
    partialStream: unknown,
    isUnidirectional: boolean
  ) {
    this.writeClosed = isUnidirectional;
    this.partialStream = partialStream;
    this.connection = connection;
  }

  /**
   * Returns if the write-end of the stream is closed. This means that it is a unidirectional stream
   */
  get writeIsClosed() {
    return this.writeClosed;
  }

  /**
   * Turn the partial stream into a full stream
   */
  initialize(options: StreamOptions): Stream {
    if (this.initialized) {
      throw new Error("Already initialized!");
    }

    const stream = lib.initialize_stream(
      this.partialStream,
      (...args: Parameters<StreamOptions["onData"]>) =>
        options.onData.apply(fullStream, args),
      (...args: Parameters<StreamOptions["onClose"]>) =>
        options.onClose.apply(fullStream, args),
      (...args: Parameters<StreamOptions["onError"]>) =>
        options.onError.apply(fullStream, args)
    );

    this.initialized = true;

    const fullStream = new Stream(this.connection, stream, this.writeClosed);

    return fullStream;
  }
}

export class Stream {
  private connection: Connection;
  private writeClosed: boolean;
  private stream: unknown;
  private details: {
    is0rtt: boolean;
    id: BigInt;
  };

  /**
   * Returns if the write-end of the stream is closed. This means that it is a unidirectional stream
   */
  get writeIsClosed() {
    return this.writeClosed;
  }

  /**
   * Get the identity of this stream
   */
  get id() {
    return this.details.id;
  }

  /**
   * Check if this stream has been opened during 0-RTT.
   *
   * In which case any non-idempotent request should be considered dangerous at the application level. Because read data is subject to replay attacks.
   */
  get is0rtt() {
    return this.details.is0rtt;
  }

  constructor(
    connection: Connection,
    stream: unknown,
    isUnidirectional: boolean
  ) {
    this.details = lib.stream_details(stream);
    this.writeClosed = isUnidirectional;
    this.connection = connection;
    this.stream = stream;
  }

  /**
   * Attempts to write the whole packet to the stream
   */
  async write(packet: Uint8Array): Promise<void> {
    if (packet.length > 0) {
      await lib.write_stream(this.stream, packet);
    }
  }

  /**
   * Closed the full stream
   */
  async close(errorCode = 0) {
    await lib.close_stream(this.stream, errorCode);
  }

  /**
   * Closed the write-end of the bidirectional stream turning it into a unidirectional stream
   */
  async closeWrite() {
    if (this.writeClosed) {
      return;
    }

    await lib.close_write(this.stream);
    this.writeClosed = true;
  }

  getConnection() {
    return this.connection;
  }
}
