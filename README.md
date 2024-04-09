# node-quic-client

This project was bootstrapped by [create-neon](https://www.npmjs.com/package/create-neon).

## Installing node-quic-client

Installing node-quic-client requires a [supported version of Node and Rust](https://github.com/neon-bindings/neon#platform-support).

You can install the project with [PNPM](https://pnpm.io). In the project directory, run:

```sh
$ pnpm install
```

This fully installs the project, including installing any dependencies and running the build.

## Building node-quic-client

If you have already installed the project and only want to run the build, run:

```sh
$ npm run build-debug
```

This command uses the [cargo-cp-artifact](https://github.com/neon-bindings/cargo-cp-artifact) utility to run the Rust build and copy the built library into `./lib/index.node`.

## Exploring node-quic-client

After building node-quic-client, you can explore its exports at the Node REPL:

```sh
$ npm install
$ node
> require('.')
```

## Available Scripts

In the project directory, you can run:

### `pnpm install`

Installs the project, including running `pnpm run build`.

### `pnpm build`

Builds the Node addon (`index.node`) from source.

Additional [`cargo build`](https://doc.rust-lang.org/cargo/commands/cargo-build.html) arguments may be passed to `pnpm build-*` commands. For example, to enable a [cargo feature](https://doc.rust-lang.org/cargo/reference/features.html):

```
pnpm run build -- --feature=beetle
```

#### `pnpm build-release`

Same as [`pnpm build-debug`](#npm-build) but, builds the module with the [`release`](https://doc.rust-lang.org/cargo/reference/profiles.html#release) profile. Release builds will compile slower, but run faster.

### Tests

Runs the unit tests by calling `cargo test`. You can learn more about [adding tests to your Rust code](https://doc.rust-lang.org/book/ch11-01-writing-tests.html) from the [Rust book](https://doc.rust-lang.org/book/).

## Learn More

To learn more about Neon, see the [Neon documentation](https://neon-bindings.com).

To learn more about Rust, see the [Rust documentation](https://www.rust-lang.org).

To learn more about Node, see the [Node documentation](https://nodejs.org).
