// Copied from https://github.com/felixbrucker/opencl-info/blob/master/bin/build-package.js

import { createReadStream, createWriteStream, promises as fs } from "node:fs";
import { pipeline } from "node:stream/promises";
import { join } from "node:path";
import archiver from "archiver";

const arch = process.argv[2] || process.arch;

const buildDir = "./build";
const nativeModulePathInZip = join("dist", "lib.node");
const nativeModulePath = join(process.cwd(), nativeModulePathInZip);

try {
  await fs.mkdir(buildDir);
} catch (_) {}

const writeStream = createWriteStream(
  join(buildDir, `${process.platform}-${arch}.tar.gz`)
);

const archive = archiver("tar", {
  gzip: true,
  gzipOptions: { level: 1 },
});

archive.append(createReadStream(nativeModulePath), {
  name: nativeModulePathInZip,
});

await archive.finalize();
await pipeline(archive, writeStream);
