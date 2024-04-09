import * as quic from "node-quic-client";

const connection = await quic.connect({
  hostname: "cloudflare.com",
  port: 443,
  alpnProtocols: ["h3"],
  onClose: (reason) => {
    console.log("Connection closed: " + reason);
  },
});

const stream = await connection.createStream({
  onError: (err) => {
    console.log("Stream error", err);
  },
  onClose: (reason) => {
    console.log("Stream closed: " + reason);
    connection.close(0).catch(console.error);
  },
  onData: (...args) => {
    console.log("Received packet", args);
    stream.close().catch(console.error);
  },
});

await stream.write(Buffer.from("Hello, World!"));
