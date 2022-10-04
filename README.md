# tokio-tungstenite-keepalive

A library to wrap a WebsocketStream in order for it to always respond to `Ping` messages
within a timely manner, without having to check for it manually, keeping connections alive.
