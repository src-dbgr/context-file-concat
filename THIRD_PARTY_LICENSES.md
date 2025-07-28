# Third Party Licenses

This document lists the third-party libraries used in Context File Concatenator and their licenses.

## Frontend Dependencies

### Monaco Editor

- **License**: MIT License
- **Source**: https://github.com/microsoft/monaco-editor
- **Copyright**: © Microsoft Corporation
- **Usage**: Code editor component for syntax highlighting and file preview

The Monaco Editor is included via CDN and is licensed under the MIT License.
See: https://github.com/microsoft/monaco-editor/blob/main/LICENSE.md

### esbuild

- **License**: MIT License
- **Source**: https://github.com/evanw/esbuild
- **Copyright**: © Evan Wallace
- **Usage**: JavaScript bundler for frontend build process

## Backend Dependencies

This project uses various Rust crates, all of which are licensed under permissive licenses (MIT, Apache-2.0, or dual-licensed). Key dependencies include:

### Core Libraries

- **wry & tao**: Cross-platform webview and windowing (MIT/Apache-2.0)
- **tokio**: Async runtime (MIT)
- **serde**: Serialization framework (MIT/Apache-2.0)
- **walkdir**: Directory traversal (MIT/Apache-2.0)

### Complete Dependency List

For a complete list of all Rust dependencies and their licenses, run:

```bash
cargo install cargo-license
cargo license
```

## License Compatibility

All third-party dependencies are compatible with the MIT License under which this project is released.
