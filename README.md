<img src="assets/icon.svg" width="64" height="64" alt="">

# forme

A fast, local-first whiteboard built in Rust and WebAssembly.

## Run locally

```bash
./build.sh
cargo build --release --bin forme-serve
./target/release/forme-serve
```

Open <http://127.0.0.1:8080>.

To edit together on other devices on the same network:

```bash
HOST=0.0.0.0 ./target/release/forme-serve
```

Use the printed LAN URL and click **Edit together live**.

## Deploy to Vercel

This repository includes `Dockerfile.vercel`, so Vercel builds the app as a
Container Function. The container builds the WASM client, bundles the server,
and serves both the static app and the `/ws` WebSocket endpoint.

```bash
npm i -g vercel
vercel link
vercel --prod
```

Vercel detects `Dockerfile.vercel` automatically. The server listens on `$PORT`
with `HOST=0.0.0.0`; the default is port 80.

### Limits

Vercel closes WebSocket connections when the Function reaches its maximum
duration (300 seconds on Hobby). The client reconnects automatically with
exponential backoff.

Rooms live in the container instance's memory. Vercel may route a new connection
to another instance, so large or long-lived rooms should use a shared store such
as Redis for cross-instance state and pub/sub.

Created in [T3 Code](https://t3.codes).
