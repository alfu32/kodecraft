Loopback TLS fixture only. `localhost-key.pem` is an intentionally public test
key, generated here for the self-signed localhost certificate. It is never used
for a live endpoint or installed into the user's trust store. TLS tests prove
that ordinary validation rejects it and only the explicit per-request bypass
accepts it. The certificate is valid for ten years from generation in October 2026.
