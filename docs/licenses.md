# Dependency & license report (§58)

CanLab itself is MIT (see `LICENSE`).

| Dependency  | Version | License          | Role                    |
|-------------|---------|------------------|-------------------------|
| clap        | 4.x     | MIT OR Apache-2.0| CLI argument parsing    |
| serde       | 1.x     | MIT OR Apache-2.0| project/trace (de)serial|
| serde_yaml  | 0.9     | MIT OR Apache-2.0| project file parsing    |
| serde_json  | 1.x     | MIT OR Apache-2.0| trace export / events   |
| thiserror   | 2.x     | MIT OR Apache-2.0| typed errors            |

All direct dependencies are permissive (MIT/Apache-2.0) and compatible
with an MIT-licensed project. No copyleft, no vendored emulator code.
Regenerate with `cargo license`-style auditing when adding dependencies;
never copy code from license-incompatible simulators — prefer APIs,
documented interfaces, and clean-room implementations.
