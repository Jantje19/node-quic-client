import * as quic from "node-quic-client";

const connection = await quic.connect({
  hostname: "cloudflare.com",
  port: 443,
  alpnProtocols: ["h3"],
  onError(err) {
    console.log("Connection error", err);
  },
  onClose(reason) {
    console.log("Connection closed: " + reason);
  },
  onStream(partialStream) {
    const stream = partialStream.initialize({
      onError: console.error,
      onClose: () => {},
      onData: () => {},
    });

    console.log("New stream. Closing immediately...");
    stream
      .close()
      .catch((err) => console.error("Error while closing the stream: " + err));
  },
});

const stream = await connection.createStream({
  onError(err) {
    console.log("Stream error", err);
  },
  onClose(reason) {
    console.log("Stream closed: " + reason);
    this.getConnection().close(0).catch(console.error);
  },
  onData(data) {
    console.log("Received packet", Buffer.from(data).toString("hex"));
  },
});

await stream.write(Buffer.from("Hello"));

console.log("Wrote");

await stream.close();
