# Dependency Analysis: Web Frameworks

This report captures the results of auditing the Rust dependency graph for the
`gst-plugin-websink` crate using Cargo's built-in inspection tooling.

## Command

```
cargo tree
```

## Observations

- The dependency tree is dominated by GStreamer bindings (`gstreamer`, `glib`),
  Tokio/futures utilities, and WebRTC-related crates.
- `warp` now appears explicitly in the dependency graph to power the embedded
  control plane. No other Rust web frameworks such as `axum`, `actix`, or
  `rocket` are included.
- The only other crate whose name contains "web" is `webbrowser`, which is
  listed under dev-dependencies and merely facilitates launching URLs during
  tests; it is not a production web framework.

## Conclusion

The build includes `warp` as the chosen HTTP server but does not pull in any
additional Rust web frameworks beyond the expected media and networking stack
required for GStreamer and WebRTC integration.
